//! Surface-surface intersection (SSI) for analytic surfaces.
//!
//! Computes the intersection curve between two parametric surfaces.
//! For analytic surface pairs (Plane, Cylinder, Cone, Sphere), many
//! intersections have known closed-form solutions.

use vcad_kernel_geom::{
    BilinearSurface, Circle3d, ConeSurface, CylinderSurface, Line3d, Plane, SphereSurface, Surface,
    SurfaceKind, TorusSurface,
};
use vcad_kernel_math::{Dir3, Point2, Point3};

/// Result of a surface-surface intersection.
#[derive(Debug, Clone)]
pub enum IntersectionCurve {
    /// No intersection.
    Empty,
    /// Single point of tangency.
    Point(Point3),
    /// Line intersection (e.g. plane-plane).
    Line(Line3d),
    /// Two parallel line intersections (e.g. plane parallel to cylinder axis).
    TwoLines(Line3d, Line3d),
    /// Circle intersection (e.g. plane-sphere, sphere-sphere).
    Circle(Circle3d),
    /// Sampled polyline for complex intersections.
    Sampled(Vec<Point3>),
    /// Two disjoint sampled curves — the typical output for two perpendicular
    /// cylinders of equal radius, whose intersection is the Steinmetz pair of
    /// ellipses. Each `Vec<Point3>` is a single closed polyline; the pipeline
    /// expands `TwoSampled` into two independent `Sampled` splits, mirroring
    /// the way `TwoLines` is unfolded.
    TwoSampled(Vec<Point3>, Vec<Point3>),
}

/// Compute the intersection of two surfaces.
///
/// Dispatches to specialized routines based on surface type.
pub fn intersect_surfaces(a: &dyn Surface, b: &dyn Surface) -> IntersectionCurve {
    match (a.surface_type(), b.surface_type()) {
        (SurfaceKind::Plane, SurfaceKind::Plane) => {
            let pa = downcast_plane(a);
            let pb = downcast_plane(b);
            match (pa, pb) {
                (Some(pa), Some(pb)) => plane_plane(pa, pb),
                _ => IntersectionCurve::Empty,
            }
        }
        (SurfaceKind::Plane, SurfaceKind::Sphere) => {
            let p = downcast_plane(a);
            let s = downcast_sphere(b);
            match (p, s) {
                (Some(p), Some(s)) => plane_sphere(p, s),
                _ => IntersectionCurve::Empty,
            }
        }
        (SurfaceKind::Sphere, SurfaceKind::Plane) => {
            let s = downcast_sphere(a);
            let p = downcast_plane(b);
            match (s, p) {
                (Some(s), Some(p)) => plane_sphere(p, s),
                _ => IntersectionCurve::Empty,
            }
        }
        (SurfaceKind::Plane, SurfaceKind::Cylinder) => {
            let p = downcast_plane(a);
            let c = downcast_cylinder(b);
            match (p, c) {
                (Some(p), Some(c)) => plane_cylinder(p, c),
                _ => IntersectionCurve::Empty,
            }
        }
        (SurfaceKind::Cylinder, SurfaceKind::Plane) => {
            let c = downcast_cylinder(a);
            let p = downcast_plane(b);
            match (c, p) {
                (Some(c), Some(p)) => plane_cylinder(p, c),
                _ => IntersectionCurve::Empty,
            }
        }
        (SurfaceKind::Plane, SurfaceKind::Cone) => {
            let p = downcast_plane(a);
            let c = downcast_cone(b);
            match (p, c) {
                (Some(p), Some(c)) => plane_cone(p, c),
                _ => IntersectionCurve::Empty,
            }
        }
        (SurfaceKind::Cone, SurfaceKind::Plane) => {
            let c = downcast_cone(a);
            let p = downcast_plane(b);
            match (c, p) {
                (Some(c), Some(p)) => plane_cone(p, c),
                _ => IntersectionCurve::Empty,
            }
        }
        (SurfaceKind::Sphere, SurfaceKind::Sphere) => {
            let sa = downcast_sphere(a);
            let sb = downcast_sphere(b);
            match (sa, sb) {
                (Some(sa), Some(sb)) => sphere_sphere(sa, sb),
                _ => IntersectionCurve::Empty,
            }
        }
        (SurfaceKind::Cylinder, SurfaceKind::Cylinder) => {
            let ca = downcast_cylinder(a);
            let cb = downcast_cylinder(b);
            match (ca, cb) {
                (Some(ca), Some(cb)) => cylinder_cylinder(ca, cb),
                _ => IntersectionCurve::Empty,
            }
        }
        // Torus intersections
        (SurfaceKind::Plane, SurfaceKind::Torus) => {
            let p = downcast_plane(a);
            let t = downcast_torus(b);
            match (p, t) {
                (Some(p), Some(t)) => plane_torus(p, t),
                _ => IntersectionCurve::Empty,
            }
        }
        (SurfaceKind::Torus, SurfaceKind::Plane) => {
            let t = downcast_torus(a);
            let p = downcast_plane(b);
            match (t, p) {
                (Some(t), Some(p)) => plane_torus(p, t),
                _ => IntersectionCurve::Empty,
            }
        }
        (SurfaceKind::Cylinder, SurfaceKind::Torus)
        | (SurfaceKind::Torus, SurfaceKind::Cylinder)
        | (SurfaceKind::Sphere, SurfaceKind::Torus)
        | (SurfaceKind::Torus, SurfaceKind::Sphere)
        | (SurfaceKind::Torus, SurfaceKind::Torus) => {
            // Complex torus intersections: use marching/sampling method
            marching_ssi(a, b, 64)
        }
        // B-spline intersections: use marching/sampling method
        (SurfaceKind::BSpline, _) | (_, SurfaceKind::BSpline) => marching_ssi(a, b, 64),
        // BilinearSurface (from sweep) — approximate as plane for fast analytic SSI
        (SurfaceKind::Bilinear, _) => {
            if let Some(plane) = downcast_bilinear(a).and_then(|b| b.to_approximate_plane()) {
                intersect_surfaces(&plane, b)
            } else {
                marching_ssi(a, b, 16)
            }
        }
        (_, SurfaceKind::Bilinear) => {
            if let Some(plane) = downcast_bilinear(b).and_then(|b| b.to_approximate_plane()) {
                intersect_surfaces(a, &plane)
            } else {
                marching_ssi(a, b, 16)
            }
        }
        _ => {
            // Unsupported pair — use marching with fewer samples.
            marching_ssi(a, b, 16)
        }
    }
}

// =============================================================================
// Downcasting helpers (safe via as_any())
// =============================================================================

fn downcast_plane(s: &dyn Surface) -> Option<&Plane> {
    s.as_any().downcast_ref::<Plane>()
}

fn downcast_sphere(s: &dyn Surface) -> Option<&SphereSurface> {
    s.as_any().downcast_ref::<SphereSurface>()
}

fn downcast_cylinder(s: &dyn Surface) -> Option<&CylinderSurface> {
    s.as_any().downcast_ref::<CylinderSurface>()
}

fn downcast_cone(s: &dyn Surface) -> Option<&ConeSurface> {
    s.as_any().downcast_ref::<ConeSurface>()
}

fn downcast_torus(s: &dyn Surface) -> Option<&TorusSurface> {
    s.as_any().downcast_ref::<TorusSurface>()
}

fn downcast_bilinear(s: &dyn Surface) -> Option<&BilinearSurface> {
    s.as_any().downcast_ref::<BilinearSurface>()
}

// =============================================================================
// Plane-Plane intersection
// =============================================================================

/// Intersection of two planes.
///
/// - Parallel + distinct → Empty
/// - Parallel + coincident → Empty (coincident faces handled in classification)
/// - Non-parallel → Line along the cross product of normals
///
/// Uses exact orient3d predicate to robustly detect coincident planes.
fn plane_plane(a: &Plane, b: &Plane) -> IntersectionCurve {
    use vcad_kernel_math::predicates::{orient3d, Sign};

    let n1 = a.normal_dir;
    let n2 = b.normal_dir;

    // Direction of intersection line = n1 × n2
    let dir = n1.as_ref().cross(n2.as_ref());
    let dir_len = dir.norm();

    if dir_len < 1e-12 {
        // Planes are parallel - use exact predicate to check if coincident
        // Generate points on each plane and test coplanarity
        let p1 = a.origin;
        let p2 = a.origin + a.x_dir.into_inner();
        let p3 = a.origin + a.y_dir.into_inner();
        let p4 = b.origin;

        // If all 4 points are coplanar (orient3d returns zero), planes are coincident
        let sign = orient3d(&p1, &p2, &p3, &p4);
        if sign == Sign::Zero {
            // Coincident — treat as empty for boolean purposes
            // (coincident faces are handled in classification, not SSI)
            return IntersectionCurve::Empty;
        }

        // Parallel but distinct
        return IntersectionCurve::Empty;
    }

    // Find a point on the intersection line.
    // Solve the system: n1 · p = d1, n2 · p = d2
    // We pick the point closest to the origin by solving in the plane
    // perpendicular to dir.
    let d1 = n1.as_ref().dot(a.origin.to_vec());
    let d2 = n2.as_ref().dot(b.origin.to_vec());

    let n1n1 = n1.as_ref().dot(n1.as_ref());
    let n1n2 = n1.as_ref().dot(n2.as_ref());
    let n2n2 = n2.as_ref().dot(n2.as_ref());

    let det = n1n1 * n2n2 - n1n2 * n1n2;
    if det.abs() < 1e-15 {
        return IntersectionCurve::Empty;
    }

    let c1 = (d1 * n2n2 - d2 * n1n2) / det;
    let c2 = (d2 * n1n1 - d1 * n1n2) / det;

    let origin = Point3::from(c1 * n1.into_inner() + c2 * n2.into_inner());

    IntersectionCurve::Line(Line3d {
        origin,
        direction: dir,
    })
}

// =============================================================================
// Plane-Sphere intersection
// =============================================================================

/// Intersection of a plane and a sphere.
///
/// - Distance > radius → Empty
/// - Distance = radius → Point (tangent)
/// - Distance < radius → Circle
fn plane_sphere(plane: &Plane, sphere: &SphereSurface) -> IntersectionCurve {
    let dist = plane.signed_distance(&sphere.center);
    let abs_dist = dist.abs();

    if abs_dist > sphere.radius + 1e-9 {
        return IntersectionCurve::Empty;
    }

    if (abs_dist - sphere.radius).abs() < 1e-9 {
        // Tangent — single point
        let point = sphere.center - dist * plane.normal_dir.into_inner();
        return IntersectionCurve::Point(point);
    }

    // Circle
    let circle_radius = (sphere.radius * sphere.radius - dist * dist).sqrt();
    let circle_center = sphere.center - dist * plane.normal_dir.into_inner();

    IntersectionCurve::Circle(Circle3d::with_normal(
        circle_center,
        circle_radius,
        *plane.normal_dir.as_ref(),
    ))
}

// =============================================================================
// Plane-Cylinder intersection
// =============================================================================

/// Intersection of a plane and a cylinder.
///
/// Three cases:
/// - Plane parallel to axis → 0, 1, or 2 lines
/// - Plane perpendicular to axis → Circle (or ellipse, but we approximate)
/// - General angle → Ellipse (we return sampled points)
fn plane_cylinder(plane: &Plane, cyl: &CylinderSurface) -> IntersectionCurve {
    let n = plane.normal_dir;
    let axis = cyl.axis;

    let cos_angle = n.as_ref().dot(axis.as_ref()).abs();

    if cos_angle < 1e-12 {
        // Plane is parallel to cylinder axis
        // Distance from cylinder axis to plane
        let axis_point = cyl.center;
        let dist = plane.signed_distance(&axis_point).abs();

        if dist > cyl.radius + 1e-9 {
            return IntersectionCurve::Empty;
        }

        if (dist - cyl.radius).abs() < 1e-9 {
            // Tangent — single line
            let closest =
                axis_point - plane.signed_distance(&axis_point) * plane.normal_dir.into_inner();
            return IntersectionCurve::Line(Line3d {
                origin: closest,
                direction: *axis.as_ref(),
            });
        }

        // Two parallel lines
        // Project axis onto plane, find the two points at distance=radius from axis
        let axis_on_plane =
            axis_point - plane.signed_distance(&axis_point) * plane.normal_dir.into_inner();

        // Handle the case where axis lies in the plane (dist ≈ 0)
        let offset = axis_on_plane - axis_point;
        let offset_len = offset.norm();

        // Find the direction perpendicular to both the plane normal and axis
        // This is the direction along which the intersection lines are offset from the axis
        let perp = if offset_len < 1e-12 {
            // Axis lies in plane - the perpendicular direction is axis × normal
            let perp = axis.as_ref().cross(plane.normal_dir.as_ref());
            if perp.norm() < 1e-12 {
                return IntersectionCurve::Empty;
            }
            perp.normalize()
        } else {
            // Normal case - perpendicular is the direction from axis to axis_on_plane
            // crossed with axis to get the tangent direction
            let offset_dir = offset / offset_len;
            offset_dir.cross(axis.as_ref()).normalize()
        };

        let lateral = (cyl.radius * cyl.radius - dist * dist).sqrt();

        let p1 = Point3::from(axis_on_plane.to_vec() + lateral * perp);
        let p2 = Point3::from(axis_on_plane.to_vec() - lateral * perp);

        // Sort the two lines deterministically so ordering doesn't depend on
        // cross-product sign (which flips with cylinder axis orientation).
        // Use lexicographic comparison on the origin coordinates.
        let (origin_a, origin_b) = {
            let cmp =
                p1.x.partial_cmp(&p2.x)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(p1.y.partial_cmp(&p2.y).unwrap_or(std::cmp::Ordering::Equal))
                    .then(p1.z.partial_cmp(&p2.z).unwrap_or(std::cmp::Ordering::Equal));
            if cmp == std::cmp::Ordering::Greater {
                (p2, p1)
            } else {
                (p1, p2)
            }
        };

        // Return both lines in deterministic order
        IntersectionCurve::TwoLines(
            Line3d {
                origin: origin_a,
                direction: *axis.as_ref(),
            },
            Line3d {
                origin: origin_b,
                direction: *axis.as_ref(),
            },
        )
    } else if (cos_angle - 1.0).abs() < 1e-12 {
        // Plane is perpendicular to cylinder axis → Circle
        let dist_along_axis =
            (plane.origin - cyl.center).dot(axis.as_ref()) / axis.as_ref().dot(axis.as_ref());
        let circle_center = cyl.center + dist_along_axis * axis.as_ref();

        // Align circle parameterization with the cylinder's ref_dir/y_dir
        // so that the circle's coordinate frame is consistent regardless of
        // plane normal orientation. Use the plane normal as the circle normal,
        // but derive x_dir from the cylinder's ref_dir.
        let circle_normal = *n.as_ref();
        let x_dir = cyl.ref_dir;
        let y_dir = Dir3::new_normalize(cyl.y_dir());
        IntersectionCurve::Circle(Circle3d {
            center: circle_center,
            radius: cyl.radius,
            x_dir,
            y_dir,
            normal: Dir3::new_normalize(circle_normal),
        })
    } else {
        // General case — ellipse
        // Sample the intersection curve
        let n_samples = 64;
        let mut points = Vec::with_capacity(n_samples);

        for i in 0..n_samples {
            let angle = 2.0 * std::f64::consts::PI * i as f64 / n_samples as f64;
            let (sin_a, cos_a) = angle.sin_cos();
            let ref_dir = cyl.ref_dir;
            let y_dir = axis.as_ref().cross(ref_dir.as_ref());

            // Point on the cylinder surface at angle `a`, arbitrary height
            let radial = cyl.radius * (cos_a * ref_dir.into_inner() + sin_a * y_dir);
            let p_on_cyl_base = cyl.center + radial;

            // Find height where this radial line intersects the plane
            // P = p_on_cyl_base + t * axis
            // plane.normal · P = plane.normal · plane.origin
            let denom = n.as_ref().dot(axis.as_ref());
            if denom.abs() < 1e-15 {
                continue;
            }
            let t = (plane.origin - p_on_cyl_base).dot(n.as_ref()) / denom;
            let intersection_point = p_on_cyl_base + t * axis.into_inner();
            points.push(intersection_point);
        }

        if points.is_empty() {
            IntersectionCurve::Empty
        } else {
            IntersectionCurve::Sampled(points)
        }
    }
}

// =============================================================================
// Plane-Cone intersection
// =============================================================================

/// Intersection of a plane and a cone.
///
/// Three cases:
/// - Plane perpendicular to cone axis → Circle (common case for cone base caps)
/// - Plane parallel to cone axis → Two lines (tangent-like case)
/// - General angle → Conic section (ellipse, parabola, hyperbola) — use sampling
fn plane_cone(plane: &Plane, cone: &ConeSurface) -> IntersectionCurve {
    let n = plane.normal_dir;
    let axis = cone.axis;

    let cos_angle = n.as_ref().dot(axis.as_ref()).abs();

    if (cos_angle - 1.0).abs() < 1e-12 {
        // Plane is perpendicular to cone axis → Circle
        // Distance along axis from apex to plane
        let apex_to_plane = (plane.origin - cone.apex).dot(axis.as_ref());

        // The cone parameterization: P(u,v) = apex + v * dir(u)
        // v > 0 is the physical cone surface; v < 0 is the phantom extension.
        // apex_to_plane tells the signed distance along the axis.
        // For the V parameter, v_param = apex_to_plane / cos(half_angle).
        // Only intersections where v_param > 0 are on the physical cone.
        let ca = cone.half_angle.cos();
        let v_param = apex_to_plane / ca;

        if v_param.abs() < 1e-12 {
            // Plane passes through the apex → Point
            return IntersectionCurve::Point(cone.apex);
        }

        if v_param < 0.0 {
            // Plane is on the phantom side of the cone (behind apex)
            return IntersectionCurve::Empty;
        }

        // Radius at this height
        let radius = apex_to_plane.abs() * cone.half_angle.tan();

        // Circle center is the projection of apex onto the plane along the axis
        let circle_center = cone.apex + apex_to_plane * axis.into_inner();

        IntersectionCurve::Circle(Circle3d::with_normal(circle_center, radius, *n.as_ref()))
    } else if cos_angle < 1e-12 {
        // Plane is parallel to cone axis
        // Use sampling for this case since computing the exact lines is complex.
        marching_ssi_cone_plane(plane, cone, 64)
    } else {
        // General case → conic section (ellipse, parabola, or hyperbola)
        // Use sampling with higher density for accuracy
        marching_ssi_cone_plane(plane, cone, 64)
    }
}

/// Sample-based SSI for plane-cone using parameter sweep.
fn marching_ssi_cone_plane(
    plane: &Plane,
    cone: &ConeSurface,
    n_samples: usize,
) -> IntersectionCurve {
    let mut points = Vec::new();

    // Sweep through U parameter (around the cone axis)
    for i in 0..n_samples {
        let u = 2.0 * std::f64::consts::PI * i as f64 / n_samples as f64;

        // For each U, find V where the cone intersects the plane.
        // P(u, v) = apex + v * (cos(α) * axis + sin(α) * (cos(u) * ref_dir + sin(u) * y_dir))
        // We need: plane.normal · (P(u,v) - plane.origin) = 0
        // plane.normal · (apex - plane.origin) + v * plane.normal · dir(u) = 0
        let (sin_u, cos_u) = u.sin_cos();
        let ca = cone.half_angle.cos();
        let sa = cone.half_angle.sin();
        let y_dir = cone.y_dir();
        let dir_u =
            ca * cone.axis.into_inner() + sa * (cos_u * cone.ref_dir.into_inner() + sin_u * y_dir);

        let denom = plane.normal_dir.as_ref().dot(dir_u);
        if denom.abs() < 1e-15 {
            continue;
        }

        let numer = (plane.origin - cone.apex).dot(plane.normal_dir.as_ref());
        let v = numer / denom;

        // Only consider points on the positive-v side of the cone
        if v > 1e-12 {
            let pt = cone.apex + v * dir_u;
            points.push(pt);
        }
    }

    if points.is_empty() {
        IntersectionCurve::Empty
    } else {
        IntersectionCurve::Sampled(points)
    }
}

// =============================================================================
// Sphere-Sphere intersection
// =============================================================================

/// Intersection of two spheres.
///
/// - Distance > r1 + r2 → Empty (too far apart)
/// - Distance < |r1 - r2| → Empty (one inside other)
/// - Distance = r1 + r2 or |r1 - r2| → Point (tangent)
/// - Otherwise → Circle
fn sphere_sphere(a: &SphereSurface, b: &SphereSurface) -> IntersectionCurve {
    let ab = b.center - a.center;
    let d = ab.norm();

    if d < 1e-12 {
        // Concentric spheres
        if (a.radius - b.radius).abs() < 1e-9 {
            // Identical — coincident
            return IntersectionCurve::Empty;
        }
        return IntersectionCurve::Empty;
    }

    if d > a.radius + b.radius + 1e-9 {
        return IntersectionCurve::Empty; // too far apart
    }

    if d < (a.radius - b.radius).abs() - 1e-9 {
        return IntersectionCurve::Empty; // one inside other
    }

    // Check tangent cases
    if (d - a.radius - b.radius).abs() < 1e-9 {
        // External tangent
        let point = a.center + (a.radius / d) * ab;
        return IntersectionCurve::Point(point);
    }

    if (d - (a.radius - b.radius).abs()).abs() < 1e-9 {
        // Internal tangent
        let point = if a.radius > b.radius {
            a.center + (a.radius / d) * ab
        } else {
            a.center - (a.radius / d) * ab
        };
        return IntersectionCurve::Point(point);
    }

    // General case — circle
    // The intersection circle lies in a plane perpendicular to the line
    // connecting the centers. Its distance from center A is:
    // h = (d² + r1² - r2²) / (2d)
    let h = (d * d + a.radius * a.radius - b.radius * b.radius) / (2.0 * d);

    let circle_center = a.center + (h / d) * ab;
    let circle_radius = (a.radius * a.radius - h * h).max(0.0).sqrt();
    let normal = Dir3::new_normalize(ab);

    IntersectionCurve::Circle(Circle3d::with_normal(
        circle_center,
        circle_radius,
        *normal.as_ref(),
    ))
}

// =============================================================================
// Cylinder-Cylinder intersection
// =============================================================================

/// Intersection of two cylinders.
///
/// Cases handled analytically:
/// - **Coaxial / parallel axes** → `Empty`. Coaxial cylinders are handled by
///   coincident-face logic in classification; for parallel-but-distinct axes
///   the SSI is either two parallel line generators or empty, but our
///   downstream consumers only call this for face pairs whose AABBs already
///   overlap, and parallel-axis cylinders that overlap will normally also
///   share planar caps that drive the trimming.
/// - **Perpendicular intersecting axes, equal radii** → two ellipses
///   (the Steinmetz curves). Emitted as `TwoSampled`. Each ellipse is a
///   closed polyline that, on cylinder A's surface, traces
///   `v = c_b ± r·cos(θ)` as θ sweeps `[0, 2π]`, where `c_b` is the height
///   of B's axis along A's axis. The two curves cross at the saddle points
///   `(u = π/2, v = c_b)` and `(u = 3π/2, v = c_b)`.
///
/// All other cylinder × cylinder geometries (skew axes, non-perpendicular
/// intersecting, perpendicular with unequal radii) fall through to
/// `marching_ssi`.
#[allow(clippy::similar_names)]
fn cylinder_cylinder(a: &CylinderSurface, b: &CylinderSurface) -> IntersectionCurve {
    use vcad_kernel_math::Vec3;

    let axis_a = a.axis.as_ref();
    let axis_b = b.axis.as_ref();
    let dot_ab = axis_a.dot(*axis_b);

    // Coaxial or parallel axes — bail out (handled elsewhere or unsupported).
    if (1.0 - dot_ab.abs()).abs() < 1e-9 {
        return IntersectionCurve::Empty;
    }

    // Only the perpendicular intersecting equal-radii case has a clean
    // analytic form. Everything else: fall through to the marching sampler
    // with a denser grid than the catch-all.
    if dot_ab.abs() > 1e-6 || (a.radius - b.radius).abs() > 1e-6 {
        return marching_ssi(a as &dyn Surface, b as &dyn Surface, 64);
    }

    // Closest points on the two axis lines. With perpendicular axes
    // (dot_ab ≈ 0) the linear system simplifies: t along axis_a, s along
    // axis_b that minimise |c_a + t·axis_a − c_b − s·axis_b|² are
    // t = (c_b − c_a)·axis_a, s = (c_a − c_b)·axis_b.
    let cb_minus_ca = b.center - a.center;
    let t = cb_minus_ca.dot(*axis_a);
    let s = -cb_minus_ca.dot(*axis_b);
    let p_a = a.center + t * (*axis_a);
    let p_b = b.center + s * (*axis_b);
    let skew_distance = (p_a - p_b).norm();

    // Skew (axes don't actually intersect) — sample.
    if skew_distance > 1e-6 {
        return marching_ssi(a as &dyn Surface, b as &dyn Surface, 64);
    }

    // Perpendicular intersecting axes, equal radii. The SSI is two ellipses
    // on planes spanned by `axis_a ± axis_b`. Parametrise each on cylinder A:
    //
    //     point(θ) = p_intersect + r·cos(θ)·ref_a + r·sin(θ)·y_a ± r·cos(θ)·axis_a
    //
    // Here `ref_a` is a unit vector perpendicular to `axis_a` chosen along
    // `axis_b` (so v=0 in cylinder-B's frame ↔ θ=π/2 on cylinder-A's surface).
    let p_intersect = p_a;
    let r = a.radius;

    // Build the orthonormal frame on cylinder A that aligns its u-axis with
    // axis_b. ref_a points along axis_b (perpendicular to axis_a since the
    // axes are perpendicular). y_a = axis_a × ref_a closes the right-handed
    // frame. With this choice θ=0 on A's surface lies on B's axis line.
    let ref_a = Vec3::new(axis_b.x, axis_b.y, axis_b.z);
    let y_a = axis_a.cross(ref_a);

    let n_samples = 64;
    let mut curve_plus = Vec::with_capacity(n_samples + 1);
    let mut curve_minus = Vec::with_capacity(n_samples + 1);
    for i in 0..=n_samples {
        let theta = 2.0 * std::f64::consts::PI * i as f64 / n_samples as f64;
        let (sin_t, cos_t) = theta.sin_cos();
        // Lateral offset on A's surface at angle θ measured from axis_b.
        let lateral = r * cos_t * ref_a + r * sin_t * y_a;
        // Axial offset along axis_a: ±r·cos(θ).
        let axial = r * cos_t * (*axis_a);
        curve_plus.push(p_intersect + lateral + axial);
        curve_minus.push(p_intersect + lateral - axial);
    }

    IntersectionCurve::TwoSampled(curve_plus, curve_minus)
}

// =============================================================================
// Plane-Torus intersection
// =============================================================================

/// Intersection of a plane and a torus.
///
/// Four cases:
/// - No intersection: plane doesn't reach the torus
/// - Tangent: single point or circle (degenerate)
/// - One circle: plane cuts through the torus once
/// - Two circles: plane cuts through outer and inner portions (Villarceau circles)
///
/// For simplicity, we use sampling for all cases since the analytic solution
/// involves quartic equations. The most common case (fillet) is plane
/// perpendicular to axis, which gives two circles.
fn plane_torus(plane: &Plane, torus: &TorusSurface) -> IntersectionCurve {
    let dist = plane.signed_distance(&torus.center).abs();
    let max_dist = torus.major_radius + torus.minor_radius;

    // Quick rejection: plane too far from torus
    if dist > max_dist + 1e-9 {
        return IntersectionCurve::Empty;
    }

    // Check if plane is perpendicular to torus axis (common case for fillets)
    let cos_angle = plane.normal_dir.as_ref().dot(torus.axis.as_ref()).abs();

    if (cos_angle - 1.0).abs() < 1e-12 {
        // Plane perpendicular to torus axis
        // The intersection is 0, 1, or 2 circles depending on distance
        let z = plane.signed_distance(&torus.center);
        let abs_z = z.abs();

        if abs_z > torus.minor_radius + 1e-9 {
            return IntersectionCurve::Empty;
        }

        if (abs_z - torus.minor_radius).abs() < 1e-9 {
            // Tangent: single circle at R from center
            let circle_center = torus.center - z * plane.normal_dir.into_inner();
            return IntersectionCurve::Circle(Circle3d::with_normal(
                circle_center,
                torus.major_radius,
                *plane.normal_dir.as_ref(),
            ));
        }

        // Two circles: inner and outer
        // r_circle = sqrt(r² - z²) is the radius contribution from the tube cross-section
        let r_offset = (torus.minor_radius * torus.minor_radius - z * z).sqrt();
        let r_outer = torus.major_radius + r_offset;
        let _r_inner = (torus.major_radius - r_offset).abs();

        let circle_center = torus.center - z * plane.normal_dir.into_inner();

        // For simplicity, return the outer circle (most relevant for filleting)
        // A more complete implementation would return both circles
        return IntersectionCurve::Circle(Circle3d::with_normal(
            circle_center,
            r_outer,
            *plane.normal_dir.as_ref(),
        ));
    }

    // General case: sample the intersection
    // The plane-torus intersection can be complex (Villarceau circles, spiric sections)
    // We use parameter-space sampling
    marching_ssi_torus_plane(plane, torus, 64)
}

/// Sample-based SSI specifically for plane-torus using UV parameter sweep.
fn marching_ssi_torus_plane(
    plane: &Plane,
    torus: &TorusSurface,
    n_samples: usize,
) -> IntersectionCurve {
    let mut points = Vec::new();

    // Sweep through U parameter (around the main axis)
    for i in 0..n_samples {
        let u = 2.0 * std::f64::consts::PI * i as f64 / n_samples as f64;

        // For each U, find V values where the torus intersects the plane
        // P(u, v) is on plane when plane.normal · (P - plane.origin) = 0
        // This is a transcendental equation in v, so we sample and find crossings

        let mut prev_dist = None;
        let n_v = 32;

        for j in 0..=n_v {
            let v = 2.0 * std::f64::consts::PI * j as f64 / n_v as f64;
            let pt = torus.evaluate(Point2::new(u, v));
            let dist = plane.signed_distance(&pt);

            if let Some(prev_d) = prev_dist {
                // Check for sign change
                if prev_d * dist < 0.0 {
                    // Refine the crossing using bisection
                    let v_prev = 2.0 * std::f64::consts::PI * (j - 1) as f64 / n_v as f64;
                    let v_refined = refine_crossing_v(torus, plane, u, v_prev, v);
                    let pt_refined = torus.evaluate(Point2::new(u, v_refined));
                    points.push(pt_refined);
                }
            }
            prev_dist = Some(dist);
        }
    }

    if points.is_empty() {
        IntersectionCurve::Empty
    } else {
        IntersectionCurve::Sampled(points)
    }
}

/// Binary search to refine the V parameter where torus crosses plane.
fn refine_crossing_v(torus: &TorusSurface, plane: &Plane, u: f64, v_a: f64, v_b: f64) -> f64 {
    let mut lo = v_a;
    let mut hi = v_b;

    for _ in 0..20 {
        let mid = 0.5 * (lo + hi);
        let pt = torus.evaluate(Point2::new(u, mid));
        let dist = plane.signed_distance(&pt);
        let pt_lo = torus.evaluate(Point2::new(u, lo));
        let dist_lo = plane.signed_distance(&pt_lo);

        if dist_lo * dist < 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }

    0.5 * (lo + hi)
}

// =============================================================================
// General marching SSI for complex surface pairs
// =============================================================================

/// Sample-based surface-surface intersection using a grid march approach.
///
/// This is used for complex surface pairs (torus-torus, B-spline, etc.)
/// where no closed-form solution exists.
fn marching_ssi(a: &dyn Surface, b: &dyn Surface, n_samples: usize) -> IntersectionCurve {
    let mut points = Vec::new();

    let ((u_min_a, u_max_a), (v_min_a, v_max_a)) = a.domain();
    // Clamp domains to reasonable bounds
    let u_min_a = u_min_a.max(-100.0);
    let u_max_a = u_max_a.min(100.0);
    let v_min_a = v_min_a.max(-100.0);
    let v_max_a = v_max_a.min(100.0);

    // Sample surface A and find closest points on surface B
    let n = n_samples;

    for i in 0..=n {
        let u = u_min_a + (u_max_a - u_min_a) * i as f64 / n as f64;
        for j in 0..=n {
            let v = v_min_a + (v_max_a - v_min_a) * j as f64 / n as f64;
            let pt_a = a.evaluate(Point2::new(u, v));

            // Find closest point on B to this point
            // Simple approach: check if distance is small
            let (closest_pt, dist) = closest_point_on_surface(b, &pt_a);

            if dist < 1e-3 {
                // Refine using Newton-Raphson or gradient descent
                let refined = refine_intersection_point(a, b, &pt_a, &closest_pt);
                if let Some(pt) = refined {
                    // Check for duplicates
                    let is_dup = points.iter().any(|p: &Point3| (*p - pt).norm() < 1e-6);
                    if !is_dup {
                        points.push(pt);
                    }
                }
            }
        }
    }

    if points.is_empty() {
        IntersectionCurve::Empty
    } else {
        // Sort points by some criterion to form a curve
        // For now, just return the sampled points
        IntersectionCurve::Sampled(points)
    }
}

/// Find the closest point on a surface to a given 3D point.
fn closest_point_on_surface(surface: &dyn Surface, target: &Point3) -> (Point3, f64) {
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();
    let u_min = u_min.max(-100.0);
    let u_max = u_max.min(100.0);
    let v_min = v_min.max(-100.0);
    let v_max = v_max.min(100.0);

    let mut best_pt = Point3::origin();
    let mut best_dist = f64::INFINITY;

    let n = 16;
    for i in 0..=n {
        let u = u_min + (u_max - u_min) * i as f64 / n as f64;
        for j in 0..=n {
            let v = v_min + (v_max - v_min) * j as f64 / n as f64;
            let pt = surface.evaluate(Point2::new(u, v));
            let dist = (pt - target).norm();
            if dist < best_dist {
                best_dist = dist;
                best_pt = pt;
            }
        }
    }

    (best_pt, best_dist)
}

/// Refine an intersection point using iterative projection.
fn refine_intersection_point(
    _a: &dyn Surface,
    _b: &dyn Surface,
    pt_a: &Point3,
    pt_b: &Point3,
) -> Option<Point3> {
    // Simple approach: return midpoint if close enough
    let mid = Point3::new(
        0.5 * (pt_a.x + pt_b.x),
        0.5 * (pt_a.y + pt_b.y),
        0.5 * (pt_a.z + pt_b.z),
    );
    let dist = (pt_a - pt_b).norm();

    if dist < 1e-2 {
        Some(mid)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::Vec3;

    #[test]
    fn test_plane_plane_perpendicular() {
        let xy = Plane::xy();
        let xz = Plane::xz();

        let result = plane_plane(&xy, &xz);
        match result {
            IntersectionCurve::Line(line) => {
                // Intersection of XY and XZ planes is the X axis
                // The direction should be along X (cross of Z and Y normals)
                assert!(line.direction.x.abs() > 0.5);
                assert!(line.direction.y.abs() < 1e-10);
                assert!(line.direction.z.abs() < 1e-10);
            }
            _ => panic!("Expected Line intersection"),
        }
    }

    #[test]
    fn test_plane_plane_parallel() {
        let a = Plane::xy();
        let b = Plane::new(Point3::new(0.0, 0.0, 5.0), Vec3::x(), Vec3::y());

        let result = plane_plane(&a, &b);
        assert!(matches!(result, IntersectionCurve::Empty));
    }

    #[test]
    fn test_plane_sphere_through_center() {
        let plane = Plane::xy();
        let sphere = SphereSurface::new(10.0); // centered at origin

        let result = plane_sphere(&plane, &sphere);
        match result {
            IntersectionCurve::Circle(circle) => {
                assert!((circle.radius - 10.0).abs() < 1e-10);
                assert!(circle.center.z.abs() < 1e-10);
            }
            _ => panic!("Expected Circle intersection"),
        }
    }

    #[test]
    fn test_plane_sphere_tangent() {
        let plane = Plane::new(Point3::new(0.0, 0.0, 10.0), Vec3::x(), Vec3::y());
        let sphere = SphereSurface::new(10.0);

        let result = plane_sphere(&plane, &sphere);
        match result {
            IntersectionCurve::Point(p) => {
                assert!((p.z - 10.0).abs() < 1e-9);
            }
            _ => panic!("Expected Point tangency, got {:?}", result),
        }
    }

    #[test]
    fn test_plane_sphere_no_intersection() {
        let plane = Plane::new(Point3::new(0.0, 0.0, 15.0), Vec3::x(), Vec3::y());
        let sphere = SphereSurface::new(10.0);

        let result = plane_sphere(&plane, &sphere);
        assert!(matches!(result, IntersectionCurve::Empty));
    }

    #[test]
    fn test_sphere_sphere_intersect() {
        let a = SphereSurface::new(10.0); // at origin
        let b = SphereSurface::with_center(Point3::new(15.0, 0.0, 0.0), 10.0);

        let result = sphere_sphere(&a, &b);
        match result {
            IntersectionCurve::Circle(circle) => {
                // Circle should be between the two centers
                assert!(circle.center.x > 0.0 && circle.center.x < 15.0);
                assert!(circle.radius > 0.0);
            }
            _ => panic!("Expected Circle intersection"),
        }
    }

    #[test]
    fn test_sphere_sphere_too_far() {
        let a = SphereSurface::new(5.0);
        let b = SphereSurface::with_center(Point3::new(100.0, 0.0, 0.0), 5.0);

        let result = sphere_sphere(&a, &b);
        assert!(matches!(result, IntersectionCurve::Empty));
    }

    #[test]
    fn test_sphere_sphere_tangent() {
        let a = SphereSurface::new(5.0);
        let b = SphereSurface::with_center(Point3::new(10.0, 0.0, 0.0), 5.0);

        let result = sphere_sphere(&a, &b);
        match result {
            IntersectionCurve::Point(p) => {
                assert!((p.x - 5.0).abs() < 1e-9);
            }
            _ => panic!("Expected Point tangency"),
        }
    }

    #[test]
    fn test_plane_cylinder_perpendicular() {
        // Plane perpendicular to Z axis, cylinder along Z
        let plane = Plane::new(Point3::new(0.0, 0.0, 5.0), Vec3::x(), Vec3::y());
        let cyl = CylinderSurface::new(10.0);

        let result = plane_cylinder(&plane, &cyl);
        match result {
            IntersectionCurve::Circle(circle) => {
                assert!((circle.radius - 10.0).abs() < 1e-10);
                assert!((circle.center.z - 5.0).abs() < 1e-10);
            }
            _ => panic!("Expected Circle intersection, got {:?}", result),
        }
    }

    #[test]
    fn test_intersect_surfaces_dispatch() {
        let a: Box<dyn Surface> = Box::new(Plane::xy());
        let b: Box<dyn Surface> = Box::new(SphereSurface::new(10.0));

        let result = intersect_surfaces(a.as_ref(), b.as_ref());
        assert!(matches!(result, IntersectionCurve::Circle(_)));
    }

    #[test]
    fn test_plane_torus_perpendicular() {
        // Plane through the center of a torus (perpendicular to axis)
        let plane = Plane::xy();
        let torus = TorusSurface::new(10.0, 3.0); // R=10, r=3

        let result = plane_torus(&plane, &torus);
        match result {
            IntersectionCurve::Circle(circle) => {
                // Outer circle should have radius R+r = 13
                assert!((circle.radius - 13.0).abs() < 1e-10);
                assert!(circle.center.z.abs() < 1e-10);
            }
            _ => panic!("Expected Circle intersection, got {:?}", result),
        }
    }

    #[test]
    fn test_plane_torus_no_intersection() {
        // Plane far from torus
        let plane = Plane::new(Point3::new(0.0, 0.0, 20.0), Vec3::x(), Vec3::y());
        let torus = TorusSurface::new(10.0, 3.0); // max extent is R+r = 13

        let result = plane_torus(&plane, &torus);
        assert!(matches!(result, IntersectionCurve::Empty));
    }

    #[test]
    fn test_plane_torus_tangent() {
        // Plane tangent to top of torus tube
        let plane = Plane::new(Point3::new(0.0, 0.0, 3.0), Vec3::x(), Vec3::y());
        let torus = TorusSurface::new(10.0, 3.0);

        let result = plane_torus(&plane, &torus);
        // Should be a circle of radius R
        match result {
            IntersectionCurve::Circle(circle) => {
                assert!((circle.radius - 10.0).abs() < 1e-10);
            }
            _ => panic!("Expected Circle intersection at tangent"),
        }
    }
}
