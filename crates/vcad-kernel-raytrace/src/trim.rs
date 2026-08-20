//! Trim testing for determining if a UV point lies within a face boundary.
//!
//! BRep faces are bounded by trim loops that define valid regions of the
//! underlying surface. This module tests whether intersection points are
//! within these boundaries.

use vcad_kernel_geom::Surface;
use vcad_kernel_math::Point2;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{FaceId, Orientation};

/// A face's trim boundary, projected into UV once and reusable for every
/// subsequent point test.
///
/// Building this is the expensive half of [`point_in_face`]: every loop
/// vertex is inverse-projected onto the surface (Newton iteration for
/// B-spline and bilinear faces), pole vertices are repaired, and degenerate
/// caps are re-synthesised from the adjacent surface. None of it depends on
/// the query point, so a ray tracer that tests millions of hits against the
/// same face should do it once — see [`FaceTrim::build`] and
/// [`FaceTrim::contains`].
#[derive(Debug, Clone)]
pub struct FaceTrim {
    /// The face spans its whole surface: the outer loop is only a seam.
    untrimmed: bool,
    /// Outer boundary in UV. Meaningless when `untrimmed`.
    outer: Vec<Point2>,
    /// For an untrimmed cylinder or cone, the extent of the loop along the
    /// unbounded `v` parameter.
    v_range: Option<(f64, f64)>,
    /// Hole boundaries in UV.
    inners: Vec<Vec<Point2>>,
}

impl FaceTrim {
    /// Project a face's trim loops into UV.
    pub fn build(brep: &BRepSolid, face_id: FaceId) -> Self {
        let topo = &brep.topology;
        let face = &topo.faces[face_id];
        let surface = &brep.geometry.surfaces[face.surface_index];

        // Get UV coordinates of the outer loop vertices
        let raw_uvs = loop_uv_coords(brep, face.outer_loop, surface.as_ref());

        // A closed surface covering the whole primitive (a full sphere or
        // torus) is bounded only by its seam: the outer loop projects to a
        // zero-area polygon in UV, which would reject every hit. Treat a
        // degenerate outer loop as "untrimmed" — the face spans the entire
        // surface — and still honour inner loops (holes) below.
        //
        // The degeneracy verdict is taken on the *raw* projection, before any
        // pole repair: a full sphere's seam loop passes through both poles,
        // and repairing those would hand it a non-zero area and reject every
        // hit.
        let mut untrimmed = polygon_area(&raw_uvs).abs() < 1e-9;
        let mut outer = if untrimmed {
            raw_uvs
        } else {
            repair_pole_vertices(surface.as_ref(), &raw_uvs)
        };

        // A planar cap bounded by a single closed circle edge (cylinder/cone
        // caps) projects to a degenerate UV polygon too — but "untrimmed" on
        // a plane means the infinite plane. Rebuild the circle from the
        // adjacent surface instead.
        if untrimmed {
            if let Some(poly) = synthesize_planar_cap_polygon(brep, face_id) {
                outer = poly;
                untrimmed = false;
            }
        }

        // On a cylinder or cone, v is an unbounded length parameter, so a
        // seam-degenerate loop (e.g. a full cylinder wall: only seam vertices
        // survive projection, the rim circles collapse) must still clamp v to
        // the loop's extent — otherwise the wall traces as an infinite
        // cylinder. u legitimately wraps the full turn.
        let v_range = if untrimmed {
            unbounded_v_range(surface.as_ref(), &outer)
        } else {
            None
        };

        let inners = face
            .inner_loops
            .iter()
            .map(|&inner_loop| loop_uv_coords(brep, inner_loop, surface.as_ref()))
            .collect();

        Self {
            untrimmed,
            outer,
            v_range,
            inners,
        }
    }

    /// Is `uv` inside the outer boundary and outside every hole?
    pub fn contains(&self, uv: Point2) -> bool {
        if self.untrimmed {
            if let Some((v_min, v_max)) = self.v_range {
                if uv.y < v_min || uv.y > v_max {
                    return false;
                }
            }
        } else if !point_in_polygon(&wrap_u_into_polygon(uv, &self.outer), &self.outer) {
            return false;
        }

        // Inside a hole is outside the face.
        !self.inners.iter().any(|inner| point_in_polygon(&uv, inner))
    }
}

/// Test if a UV point is inside a face's trim boundaries.
///
/// Returns `true` if the point is inside the outer loop and outside all inner loops (holes).
///
/// This reprojects the face's loops on every call. Callers testing the same
/// face repeatedly — the ray tracer, above all — should hold a [`FaceTrim`]
/// instead.
pub fn point_in_face(brep: &BRepSolid, face_id: FaceId, uv: Point2) -> bool {
    FaceTrim::build(brep, face_id).contains(uv)
}

/// Latitude magnitude above which a spherical loop vertex counts as sitting
/// on a pole, where longitude is indeterminate.
const POLE_V_EPS: f64 = 1e-7;

/// Repair a spherical face's UV trim polygon around pole vertices and the
/// longitude seam.
///
/// On a sphere, `u` (longitude) is meaningless at a pole: every meridian
/// meets there, and `project_to_sphere` reports `u = 0` for want of anything
/// better. Straight-line point-in-polygon then draws the boundary to
/// `u = 0` instead of up the two meridians that actually bound the patch.
///
/// The fillet corner blend is exactly this case. Each convex corner of a
/// filleted box gets a spherical octant whose three vertices are the
/// tangency points with the three faces; the first of those lands on the
/// pole (the surface's `axis` is that face's normal), so the loop projects
/// to the right triangle `(0, π/2), (0, 0), (π/2, 0)` when the true patch is
/// the *square* `[0, π/2] × [0, π/2]`. The hypotenuse cuts away half the
/// octant, and rays through the missing half pass into the solid — the
/// crescent-shaped hole visible at every corner of a ray-traced filleted
/// part, right where the three fillet cylinders meet.
///
/// The repair replaces each pole vertex with two vertices carrying the
/// longitudes of its neighbours, so the boundary runs up one meridian,
/// across the pole, and back down the other. Longitudes are also unwrapped
/// onto one continuous branch first, so a patch straddling the `u = 0` seam
/// stays a single polygon instead of folding across the whole domain.
///
/// Non-spherical surfaces are returned unchanged.
fn repair_pole_vertices(surface: &dyn Surface, uvs: &[Point2]) -> Vec<Point2> {
    use vcad_kernel_geom::SurfaceKind;
    if surface.surface_type() != SurfaceKind::Sphere || uvs.len() < 3 {
        return uvs.to_vec();
    }
    let half_pi = std::f64::consts::FRAC_PI_2;
    let is_pole = |uv: &Point2| (uv.y.abs() - half_pi).abs() < POLE_V_EPS;

    // Nothing to do when the loop avoids both poles — but still unwrap, so a
    // seam-straddling patch tests as one contiguous polygon.
    let n = uvs.len();
    let anchor = (0..n).find(|&i| !is_pole(&uvs[i]));
    let Some(anchor) = anchor else {
        return uvs.to_vec();
    };

    // Unwrap longitudes of the non-pole vertices onto a continuous branch,
    // walking the loop from the anchor so each step takes the shortest turn.
    let tau = std::f64::consts::TAU;
    let mut unwrapped: Vec<Option<f64>> = vec![None; n];
    unwrapped[anchor] = Some(uvs[anchor].x);
    let mut last = uvs[anchor].x;
    for step in 1..n {
        let i = (anchor + step) % n;
        if is_pole(&uvs[i]) {
            continue;
        }
        let mut u = uvs[i].x;
        while u - last > std::f64::consts::PI {
            u -= tau;
        }
        while last - u > std::f64::consts::PI {
            u += tau;
        }
        unwrapped[i] = Some(u);
        last = u;
    }

    // Emit, expanding each pole vertex into the two meridian endpoints that
    // bracket it in loop order.
    let prev_nonpole = |i: usize| -> f64 {
        (1..=n)
            .find_map(|k| unwrapped[(i + n - k) % n])
            .unwrap_or(0.0)
    };
    let next_nonpole =
        |i: usize| -> f64 { (1..=n).find_map(|k| unwrapped[(i + k) % n]).unwrap_or(0.0) };

    let mut out = Vec::with_capacity(n + 2);
    for i in 0..n {
        if let Some(u) = unwrapped[i] {
            out.push(Point2::new(u, uvs[i].y));
            continue;
        }
        let v = if uvs[i].y > 0.0 { half_pi } else { -half_pi };
        out.push(Point2::new(prev_nonpole(i), v));
        out.push(Point2::new(next_nonpole(i), v));
    }
    out
}

/// Shift a query longitude onto the branch spanned by a repaired polygon.
///
/// [`repair_pole_vertices`] unwraps longitudes, so a polygon may live on
/// `[-π/2, π/2]` while `project_to_sphere` reports the hit at `3π/2`. The
/// polygon's `u` extent is always under a full turn, so at most one whole-turn
/// shift can land inside.
fn wrap_u_into_polygon(uv: Point2, polygon: &[Point2]) -> Point2 {
    let tau = std::f64::consts::TAU;
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in polygon {
        lo = lo.min(p.x);
        hi = hi.max(p.x);
    }
    if !lo.is_finite() || hi - lo >= tau {
        return uv;
    }
    // Map u into [lo, lo + τ). Since the polygon spans under a full turn,
    // that interval contains [lo, hi] and any u inside the patch lands there.
    let u = uv.x - ((uv.x - lo) / tau).floor() * tau;
    Point2::new(u, uv.y)
}

/// Signed area of a UV polygon (shoelace). Near-zero means the loop is
/// degenerate in parameter space — e.g. a closed surface's seam-only loop.
pub(crate) fn polygon_area(polygon: &[Point2]) -> f64 {
    if polygon.len() < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    for i in 0..polygon.len() {
        let a = &polygon[i];
        let b = &polygon[(i + 1) % polygon.len()];
        area += a.x * b.y - b.x * a.y;
    }
    area / 2.0
}

/// For surfaces whose v parameter is an unbounded length (cylinder, cone),
/// return the v-range spanned by a degenerate outer loop's vertices. Returns
/// `None` for surfaces with intrinsically bounded v (sphere, torus, plane…),
/// where the untrimmed fallback is safe as-is, or when the loop is empty.
pub(crate) fn unbounded_v_range(surface: &dyn Surface, outer_uvs: &[Point2]) -> Option<(f64, f64)> {
    use vcad_kernel_geom::SurfaceKind;
    match surface.surface_type() {
        SurfaceKind::Cylinder | SurfaceKind::Cone => {
            let mut it = outer_uvs.iter();
            let first = it.next()?;
            let (mut v_min, mut v_max) = (first.y, first.y);
            for uv in it {
                v_min = v_min.min(uv.y);
                v_max = v_max.max(uv.y);
            }
            Some((v_min, v_max))
        }
        _ => None,
    }
}

/// Number of segments used when a degenerate cap loop is rebuilt as a
/// sampled circle polygon.
const CAP_CIRCLE_SEGMENTS: usize = 64;

/// Rebuild the trim polygon for a planar face whose outer loop is degenerate
/// in UV — a cap bounded by a single closed circle edge (e.g. a cylinder or
/// cone cap). Only loop *vertices* project to UV, so a full-circle edge
/// collapses to a point and the cap would either be rejected (GPU) or spill
/// to the whole infinite plane (CPU untrimmed fallback).
///
/// The circle is recovered from the adjacent surface across the loop's twin
/// half-edge: the surface's axis pierced through the cap plane gives the
/// center, and the loop vertex gives the radius. Returns a sampled polygon
/// in the plane's (orthonormal, hence isometric) UV space, or `None` when
/// the face isn't planar or no axis-bearing neighbour is found.
pub(crate) fn synthesize_planar_cap_polygon(
    brep: &BRepSolid,
    face_id: FaceId,
) -> Option<Vec<Point2>> {
    use vcad_kernel_geom::SurfaceKind;

    let topo = &brep.topology;
    let face = &topo.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    let plane = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>()?;

    for he_id in topo.loop_half_edges(face.outer_loop) {
        let he = &topo.half_edges[he_id];
        let Some(twin_id) = he.twin else { continue };
        let Some(loop_id) = topo.half_edges[twin_id].loop_id else {
            continue;
        };
        let Some(nbr_face_id) = topo.loops[loop_id].face else {
            continue;
        };
        let nbr_surface = &brep.geometry.surfaces[topo.faces[nbr_face_id].surface_index];

        // Axis (point + direction) of the neighbouring surface of revolution.
        let (axis_point, axis_dir) = match nbr_surface.surface_type() {
            SurfaceKind::Cylinder => {
                let cyl = nbr_surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::CylinderSurface>()?;
                (cyl.center, *cyl.axis.as_ref())
            }
            SurfaceKind::Cone => {
                let cone = nbr_surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::ConeSurface>()?;
                (cone.apex, *cone.axis.as_ref())
            }
            SurfaceKind::Sphere => {
                let sph = nbr_surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::SphereSurface>()?;
                (sph.center, *sph.axis.as_ref())
            }
            SurfaceKind::Torus => {
                let torus = nbr_surface
                    .as_any()
                    .downcast_ref::<vcad_kernel_geom::TorusSurface>()?;
                (torus.center, *torus.axis.as_ref())
            }
            _ => continue,
        };

        // Pierce the axis through the cap plane to get the circle center;
        // if the axis is parallel to the plane, project the axis point.
        let n = plane.normal_dir.as_ref();
        let denom = axis_dir.dot(n);
        let center = if denom.abs() > 1e-12 {
            let t = (plane.origin - axis_point).dot(n) / denom;
            axis_point + axis_dir * t
        } else {
            axis_point - *n * (axis_point - plane.origin).dot(n)
        };

        let vertex = topo.vertices[he.origin].point;
        let radius = (vertex - center).norm();
        if radius < 1e-12 {
            continue;
        }

        let center_uv = plane.project(&center);
        let mut poly = Vec::with_capacity(CAP_CIRCLE_SEGMENTS);
        for i in 0..CAP_CIRCLE_SEGMENTS {
            let theta = std::f64::consts::TAU * (i as f64) / (CAP_CIRCLE_SEGMENTS as f64);
            poly.push(Point2::new(
                center_uv.x + radius * theta.cos(),
                center_uv.y + radius * theta.sin(),
            ));
        }
        return Some(poly);
    }

    None
}

/// Project a 3D point onto the parameter space of a face's surface.
///
/// The inverse of evaluating the face's surface — the coordinates
/// [`point_in_face`] expects. Exposed so callers can ask whether a specific
/// surface point falls inside a face's trim boundary.
pub fn project_face_uv(
    brep: &BRepSolid,
    face_id: FaceId,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    project_to_surface_uv(surface.as_ref(), point)
}

/// Get the UV coordinates of vertices in a loop by projecting 3D positions onto the surface.
fn loop_uv_coords(
    brep: &BRepSolid,
    loop_id: vcad_kernel_topo::LoopId,
    surface: &dyn Surface,
) -> Vec<Point2> {
    let topo = &brep.topology;

    topo.loop_half_edges(loop_id)
        .map(|he_id| {
            let v_id = topo.half_edges[he_id].origin;
            let point = topo.vertices[v_id].point;
            project_to_surface_uv(surface, &point)
        })
        .collect()
}

/// Project a 3D point onto a surface's UV parameter space.
///
/// This is an inverse evaluation that finds (u, v) such that surface.evaluate(u, v) ≈ point.
fn project_to_surface_uv(surface: &dyn Surface, point: &vcad_kernel_math::Point3) -> Point2 {
    use vcad_kernel_geom::SurfaceKind;

    match surface.surface_type() {
        SurfaceKind::Plane => {
            if let Some(plane) = surface.as_any().downcast_ref::<vcad_kernel_geom::Plane>() {
                return plane.project(point);
            }
        }
        SurfaceKind::Cylinder => {
            if let Some(cyl) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::CylinderSurface>()
            {
                return project_to_cylinder(cyl, point);
            }
        }
        SurfaceKind::Sphere => {
            if let Some(sph) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::SphereSurface>()
            {
                return project_to_sphere(sph, point);
            }
        }
        SurfaceKind::Cone => {
            if let Some(cone) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::ConeSurface>()
            {
                return project_to_cone(cone, point);
            }
        }
        SurfaceKind::Torus => {
            if let Some(torus) = surface
                .as_any()
                .downcast_ref::<vcad_kernel_geom::TorusSurface>()
            {
                return project_to_torus(torus, point);
            }
        }
        _ => {}
    }

    // Fallback: Newton iteration for general surfaces
    project_newton(surface, point)
}

fn project_to_cylinder(
    cyl: &vcad_kernel_geom::CylinderSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = cyl.axis.as_ref();
    let ref_dir = cyl.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = point - cyl.center;
    let v = to_point.dot(axis);

    let proj = to_point - v * axis;
    let x = proj.dot(ref_dir);
    let y = proj.dot(y_dir);

    let u = y.atan2(x);
    let u = if u < 0.0 { u + 2.0 * PI } else { u };

    Point2::new(u, v)
}

fn project_to_sphere(
    sph: &vcad_kernel_geom::SphereSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = sph.axis.as_ref();
    let ref_dir = sph.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = (point - sph.center) / sph.radius;
    let z = to_point.dot(axis).clamp(-1.0, 1.0);
    let v = z.asin();

    let proj = to_point - z * axis;
    let proj_len = proj.norm();

    let u = if proj_len > 1e-12 {
        let x = proj.dot(ref_dir) / proj_len;
        let y = proj.dot(y_dir) / proj_len;
        let angle = y.atan2(x);
        if angle < 0.0 {
            angle + 2.0 * PI
        } else {
            angle
        }
    } else {
        0.0
    };

    Point2::new(u, v)
}

fn project_to_cone(
    cone: &vcad_kernel_geom::ConeSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = cone.axis.as_ref();
    let ref_dir = cone.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);
    let cos_a = cone.half_angle.cos();

    let to_point = point - cone.apex;
    let height = to_point.dot(axis);
    let v = height / cos_a;

    let proj = to_point - height * axis;
    let proj_len = proj.norm();

    let u = if proj_len > 1e-12 {
        let x = proj.dot(ref_dir) / proj_len;
        let y = proj.dot(y_dir) / proj_len;
        let angle = y.atan2(x);
        if angle < 0.0 {
            angle + 2.0 * PI
        } else {
            angle
        }
    } else {
        0.0
    };

    Point2::new(u, v)
}

fn project_to_torus(
    torus: &vcad_kernel_geom::TorusSurface,
    point: &vcad_kernel_math::Point3,
) -> Point2 {
    use std::f64::consts::PI;

    let axis = torus.axis.as_ref();
    let ref_dir = torus.ref_dir.as_ref();
    let y_dir = axis.cross(ref_dir);

    let to_point = point - torus.center;
    let h = to_point.dot(axis);

    let proj = to_point - h * axis;
    let proj_len = proj.norm();

    let u = if proj_len > 1e-12 {
        let x = proj.dot(ref_dir);
        let y = proj.dot(y_dir);
        let angle = y.atan2(x);
        if angle < 0.0 {
            angle + 2.0 * PI
        } else {
            angle
        }
    } else {
        0.0
    };

    let tube_center_dist = proj_len - torus.major_radius;
    let v = h.atan2(tube_center_dist);
    let v = if v < 0.0 { v + 2.0 * PI } else { v };

    Point2::new(u, v)
}

/// Newton iteration to find UV coordinates for a point on a surface.
fn project_newton(surface: &dyn Surface, point: &vcad_kernel_math::Point3) -> Point2 {
    let ((u_min, u_max), (v_min, v_max)) = surface.domain();
    let mut uv = Point2::new((u_min + u_max) / 2.0, (v_min + v_max) / 2.0);

    for _ in 0..20 {
        let p = surface.evaluate(uv);
        let du = surface.d_du(uv);
        let dv = surface.d_dv(uv);

        let residual = p - point;

        // Solve 2x2 system: [du, dv]^T * [delta_u, delta_v]^T = residual
        // Using least squares: (J^T J) * delta = J^T * residual
        let a11 = du.dot(du);
        let a12 = du.dot(dv);
        let a22 = dv.dot(dv);
        let b1 = du.dot(residual);
        let b2 = dv.dot(residual);

        let det = a11 * a22 - a12 * a12;
        if det.abs() < 1e-14 {
            break;
        }

        let delta_u = (a22 * b1 - a12 * b2) / det;
        let delta_v = (a11 * b2 - a12 * b1) / det;

        uv.x -= delta_u;
        uv.y -= delta_v;

        // Clamp to domain
        uv.x = uv.x.clamp(u_min, u_max);
        uv.y = uv.y.clamp(v_min, v_max);

        if delta_u.abs() < 1e-10 && delta_v.abs() < 1e-10 {
            break;
        }
    }

    uv
}

/// Point-in-polygon test using the winding number algorithm.
///
/// Works correctly for both convex and concave polygons.
pub fn point_in_polygon(point: &Point2, polygon: &[Point2]) -> bool {
    if polygon.len() < 3 {
        return false;
    }

    let mut winding = 0i32;
    let n = polygon.len();

    for i in 0..n {
        let p1 = polygon[i];
        let p2 = polygon[(i + 1) % n];

        if p1.y <= point.y {
            if p2.y > point.y {
                // Upward crossing
                if is_left(&p1, &p2, point) > 0.0 {
                    winding += 1;
                }
            }
        } else if p2.y <= point.y {
            // Downward crossing
            if is_left(&p1, &p2, point) < 0.0 {
                winding -= 1;
            }
        }
    }

    winding != 0
}

/// Compute the signed area of the triangle (p0, p1, p2).
/// Positive if p2 is to the left of the line p0->p1.
#[inline]
fn is_left(p0: &Point2, p1: &Point2, p2: &Point2) -> f64 {
    (p1.x - p0.x) * (p2.y - p0.y) - (p2.x - p0.x) * (p1.y - p0.y)
}

/// Extract the UV coordinates of a face's outer loop.
///
/// This returns the UV coordinates of the outer boundary loop vertices,
/// suitable for point-in-polygon testing.
pub fn extract_face_uv_loop(brep: &BRepSolid, face_id: FaceId) -> Vec<Point2> {
    let topo = &brep.topology;
    let face = &topo.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    let raw = loop_uv_coords(brep, face.outer_loop, surface.as_ref());
    // Mirror `point_in_face`: repair pole longitudes on a real trim loop, but
    // leave a degenerate (seam-only) loop alone so callers can still detect
    // it and fall back to "untrimmed".
    if polygon_area(&raw).abs() < 1e-9 {
        raw
    } else {
        repair_pole_vertices(surface.as_ref(), &raw)
    }
}

/// Extract UV coordinates for all inner loops (holes) of a face.
///
/// Returns a vector of inner loops, where each inner loop is a vector of UV points.
pub fn extract_face_inner_loops(brep: &BRepSolid, face_id: FaceId) -> Vec<Vec<Point2>> {
    let topo = &brep.topology;
    let face = &topo.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];

    face.inner_loops
        .iter()
        .map(|&loop_id| loop_uv_coords(brep, loop_id, surface.as_ref()))
        .collect()
}

/// Get the face normal considering orientation.
pub fn face_normal(brep: &BRepSolid, face_id: FaceId, uv: Point2) -> vcad_kernel_math::Dir3 {
    let face = &brep.topology.faces[face_id];
    let surface = &brep.geometry.surfaces[face.surface_index];
    let n = surface.normal(uv);

    match face.orientation {
        Orientation::Forward => n,
        Orientation::Reversed => vcad_kernel_math::Dir3::new_normalize(-n.into_inner()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_primitives::make_cube;

    #[test]
    fn test_point_in_polygon_square() {
        let square = vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
            Point2::new(0.0, 1.0),
        ];

        assert!(point_in_polygon(&Point2::new(0.5, 0.5), &square));
        assert!(point_in_polygon(&Point2::new(0.1, 0.1), &square));
        assert!(!point_in_polygon(&Point2::new(1.5, 0.5), &square));
        assert!(!point_in_polygon(&Point2::new(-0.1, 0.5), &square));
    }

    #[test]
    fn test_point_in_polygon_concave() {
        // L-shaped polygon
        let l_shape = vec![
            Point2::new(0.0, 0.0),
            Point2::new(2.0, 0.0),
            Point2::new(2.0, 1.0),
            Point2::new(1.0, 1.0),
            Point2::new(1.0, 2.0),
            Point2::new(0.0, 2.0),
        ];

        assert!(point_in_polygon(&Point2::new(0.5, 0.5), &l_shape));
        assert!(point_in_polygon(&Point2::new(0.5, 1.5), &l_shape));
        assert!(!point_in_polygon(&Point2::new(1.5, 1.5), &l_shape)); // In the concave notch
    }

    #[test]
    fn test_point_in_cube_face() {
        let cube = make_cube(10.0, 10.0, 10.0);

        // Get the first face and test a point in the middle
        let face_id = cube.topology.faces.iter().next().unwrap().0;

        // The face should be a 10x10 square in some plane
        // UV coordinates depend on the face, but center should be valid
        let center_uv = Point2::new(5.0, 5.0);
        assert!(point_in_face(&cube, face_id, center_uv));
    }

    #[test]
    fn test_project_to_cylinder() {
        use std::f64::consts::PI;
        let cyl = vcad_kernel_geom::CylinderSurface::new(5.0);

        // Point at (5, 0, 3) should project to u=0, v=3
        let uv = project_to_cylinder(&cyl, &vcad_kernel_math::Point3::new(5.0, 0.0, 3.0));
        assert!(uv.x.abs() < 1e-10);
        assert!((uv.y - 3.0).abs() < 1e-10);

        // Point at (0, 5, 7) should project to u=π/2, v=7
        let uv2 = project_to_cylinder(&cyl, &vcad_kernel_math::Point3::new(0.0, 5.0, 7.0));
        assert!((uv2.x - PI / 2.0).abs() < 1e-10);
        assert!((uv2.y - 7.0).abs() < 1e-10);
    }
}
