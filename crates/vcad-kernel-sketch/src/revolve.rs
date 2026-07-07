//! Revolve operation: create a solid by rotating a profile around an axis.

use std::collections::HashMap;
use std::f64::consts::PI;

use vcad_kernel_geom::{CylinderSurface, GeometryStore, Plane};
use vcad_kernel_math::{Dir3, Point3, Tolerance, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{HalfEdgeId, Orientation, ShellType, Topology, VertexId};

use crate::{SketchError, SketchProfile, SketchSegment};

/// Revolve a closed profile around an axis to create a B-rep solid.
///
/// # Arguments
///
/// * `profile` - The closed 2D profile to revolve (must be line-only for now)
/// * `axis_origin` - A point on the axis of revolution
/// * `axis_dir` - Direction of the axis of revolution
/// * `angle` - Angle of revolution in radians (must be in (0, 2π])
///
/// # Returns
///
/// A B-rep solid with surfaces of revolution.
///
/// # Errors
///
/// - `ZeroAxis` if the axis direction is zero
/// - `InvalidAngle` if angle is not in (0, 2π]
/// - `ArcNotSupported` if the profile contains arc segments
/// - `AxisIntersection` if any profile vertex lies on the axis
///
/// # Current Limitations
///
/// Arc segments in the profile would produce torus surfaces, which are not
/// yet supported. Use line-only profiles.
///
/// # Example
///
/// ```
/// use vcad_kernel_sketch::{SketchProfile, revolve};
/// use vcad_kernel_math::{Point3, Vec3};
/// use std::f64::consts::PI;
///
/// // Create a rectangle profile offset from the axis
/// let profile = SketchProfile::rectangle(
///     Point3::new(5.0, 0.0, 0.0),  // Offset from Y axis
///     Vec3::x(),
///     Vec3::z(),
///     3.0, 10.0,
/// );
///
/// // Revolve 360° around Y axis → hollow cylinder
/// let solid = revolve(&profile, Point3::origin(), Vec3::y(), 2.0 * PI).unwrap();
/// ```
pub fn revolve(
    profile: &SketchProfile,
    axis_origin: Point3,
    axis_dir: Vec3,
    angle: f64,
) -> Result<BRepSolid, SketchError> {
    // Validate axis
    if axis_dir.norm() < 1e-12 {
        return Err(SketchError::ZeroAxis);
    }
    let axis = Dir3::new_normalize(axis_dir);

    // Validate angle
    if angle <= 0.0 || angle > 2.0 * PI + 1e-9 {
        return Err(SketchError::InvalidAngle(angle));
    }

    // Check for arc segments (not supported)
    if !profile.is_line_only() {
        return Err(SketchError::ArcNotSupported);
    }

    let tol = Tolerance::DEFAULT;
    let is_full = (angle - 2.0 * PI).abs() < 1e-9;

    // Validate profile doesn't intersect axis
    for seg in &profile.segments {
        let p = profile.to_3d(seg.start());
        let dist = point_to_line_distance(&p, &axis_origin, axis.as_ref());
        if dist < tol.linear {
            return Err(SketchError::AxisIntersection);
        }
    }

    let mut topo = Topology::new();
    let mut geom = GeometryStore::new();

    // Vertex cache
    let mut vertex_cache: HashMap<[i64; 3], VertexId> = HashMap::new();

    let quantize_pt = |p: Point3| -> [i64; 3] {
        [
            (p.x * 1e9).round() as i64,
            (p.y * 1e9).round() as i64,
            (p.z * 1e9).round() as i64,
        ]
    };

    let get_or_create_vertex =
        |cache: &mut HashMap<[i64; 3], VertexId>, topo: &mut Topology, pos: Point3| -> VertexId {
            let key = quantize_pt(pos);
            *cache.entry(key).or_insert_with(|| topo.add_vertex(pos))
        };

    let n_segments = profile.segments.len();

    // Per-vertex (radius, axial) coordinates and the profile's winding in
    // the (r, t) half-plane. The winding sign orients every generated
    // face: for a counterclockwise profile (positive shoelace area), the
    // outward normal of the surface swept by a segment with direction
    // (dr, dt) is (dt·radial − dr·axis).
    let rt: Vec<(f64, f64)> = profile
        .segments
        .iter()
        .map(|seg| {
            let p = profile.to_3d(seg.start());
            let t = (p - axis_origin).dot(axis.as_ref());
            let proj = axis_origin + t * axis.as_ref();
            ((p - proj).norm(), t)
        })
        .collect();
    let mut shoelace = 0.0;
    for i in 0..n_segments {
        let (r0, t0) = rt[i];
        let (r1, t1) = rt[(i + 1) % n_segments];
        shoelace += r0 * t1 - r1 * t0;
    }
    let ccw = shoelace > 0.0;

    // Angular facet count for swept faces that have no analytic surface
    // (partial revolutions, cones): one facet per ~11.25° matches the
    // default 32-segment circle tessellation.
    let n_steps = ((angle / (2.0 * PI) * 32.0).ceil() as usize).max(1);

    // Vertex rings: rings[j][i] = profile vertex i rotated by j·angle/n.
    // Full revolutions only need ring 0 (the seam).
    let n_rings = if is_full { 1 } else { n_steps + 1 };
    let mut rings: Vec<Vec<VertexId>> = Vec::with_capacity(n_rings);
    for j in 0..n_rings {
        let theta = angle * j as f64 / n_steps as f64;
        let ring = profile
            .segments
            .iter()
            .map(|seg| {
                let p = profile.to_3d(seg.start());
                let pr = rotate_point(&p, &axis_origin, axis.as_ref(), theta);
                get_or_create_vertex(&mut vertex_cache, &mut topo, pr)
            })
            .collect();
        rings.push(ring);
    }
    let start_verts: Vec<VertexId> = rings[0].clone();
    let end_verts: Vec<VertexId> = rings[n_rings - 1].clone();

    let mut all_faces = Vec::new();
    let mut he_map: HashMap<([i64; 3], [i64; 3]), HalfEdgeId> = HashMap::new();

    // Build revolution faces for each line segment
    for (i, seg) in profile.segments.iter().enumerate() {
        let next_i = (i + 1) % n_segments;

        let SketchSegment::Line { start, end } = seg else {
            unreachable!("already checked line-only");
        };

        let p_start = profile.to_3d(*start);
        let p_end = profile.to_3d(*end);

        // Classify the line segment relative to the axis
        let surf_type = classify_line_segment(&p_start, &p_end, &axis_origin, axis.as_ref());
        let (dr, dt) = (rt[next_i].0 - rt[i].0, rt[next_i].1 - rt[i].1);

        if is_full {
            match surf_type {
                RevolveSurfaceType::Cylinder { radius } => {
                    // Outward radial ⇔ the profile walks up (+t) on a CCW
                    // profile; otherwise the wall faces the axis.
                    let outward = if ccw { dt > 0.0 } else { dt < 0.0 };
                    all_faces.push(build_full_cylinder_face(
                        &mut topo,
                        &mut geom,
                        &axis_origin,
                        axis.as_ref(),
                        radius,
                        &start_verts[i],
                        &start_verts[next_i],
                        outward,
                        &mut he_map,
                        quantize_pt,
                    ));
                }
                RevolveSurfaceType::Plane { .. } => {
                    // Annular planar face: degenerate outer circle loop plus
                    // a densely sampled inner hole.
                    let axis_sign = if ccw { -dr.signum() } else { dr.signum() };
                    all_faces.push(build_full_annular_face(
                        &mut topo,
                        &mut geom,
                        &mut vertex_cache,
                        &axis_origin,
                        axis.as_ref(),
                        rt[i],
                        rt[next_i],
                        &start_verts[i],
                        &start_verts[next_i],
                        axis_sign,
                        quantize_pt,
                    ));
                }
                RevolveSurfaceType::Cone { .. } => {
                    // Faceted ring (true cone surfaces TODO).
                    for j in 0..n_steps {
                        let theta0 = angle * j as f64 / n_steps as f64;
                        let theta1 = angle * (j + 1) as f64 / n_steps as f64;
                        all_faces.push(build_facet_quad(
                            &mut topo,
                            &mut geom,
                            &mut vertex_cache,
                            &axis_origin,
                            axis.as_ref(),
                            &p_start,
                            &p_end,
                            theta0,
                            theta1,
                            ccw,
                            &mut he_map,
                            quantize_pt,
                        ));
                    }
                }
            }
        } else {
            // Partial revolution: faceted angular sweep. A single flat
            // quad spanning the whole angle (the previous implementation)
            // collapses to zero volume at 180° and misrepresents every
            // other angle.
            for j in 0..n_steps {
                let theta0 = angle * j as f64 / n_steps as f64;
                let theta1 = angle * (j + 1) as f64 / n_steps as f64;
                all_faces.push(build_facet_quad(
                    &mut topo,
                    &mut geom,
                    &mut vertex_cache,
                    &axis_origin,
                    axis.as_ref(),
                    &p_start,
                    &p_end,
                    theta0,
                    theta1,
                    ccw,
                    &mut he_map,
                    quantize_pt,
                ));
            }
        }
    }

    // For partial revolution, add closing side faces. Their loop winding
    // must run opposite to the adjacent facet-quad edges so twins pair,
    // which depends on the profile winding (see build_facet_quad).
    if !is_full {
        // For a CCW profile the facet quads' θ=0 edge runs next_i → i, so
        // the start cap walks the profile forward (not reversed); at
        // θ=angle the quad edge runs i → next_i, so the end cap reverses.
        let start_face = build_side_face(
            &mut topo,
            &mut geom,
            &start_verts,
            &mut he_map,
            quantize_pt,
            !ccw,
        );
        all_faces.push(start_face);

        let end_face = build_side_face(
            &mut topo,
            &mut geom,
            &end_verts,
            &mut he_map,
            quantize_pt,
            ccw,
        );
        all_faces.push(end_face);
    }

    // Pair twin half-edges
    pair_twin_half_edges(&mut topo, &he_map);

    // Build shell and solid
    let shell = topo.add_shell(all_faces, ShellType::Outer);
    let solid_id = topo.add_solid(shell);

    Ok(BRepSolid {
        topology: topo,
        geometry: geom,
        solid_id,
    })
}

/// Classification of a line segment for revolve surface type.
#[derive(Debug)]
#[allow(dead_code)]
enum RevolveSurfaceType {
    /// Line parallel to axis at distance r → cylinder
    Cylinder { radius: f64 },
    /// Line perpendicular to axis → annular plane
    Plane { height: f64 },
    /// Line at angle to axis → cone
    Cone { apex: Point3, half_angle: f64 },
}

fn classify_line_segment(
    p_start: &Point3,
    p_end: &Point3,
    axis_origin: &Point3,
    axis: &Vec3,
) -> RevolveSurfaceType {
    let tol = Tolerance::DEFAULT;

    // Project points onto axis
    let t_start = (p_start - axis_origin).dot(axis);
    let t_end = (p_end - axis_origin).dot(axis);

    // Radial distances
    let proj_start = *axis_origin + t_start * axis;
    let proj_end = *axis_origin + t_end * axis;
    let r_start = (p_start - proj_start).norm();
    let r_end = (p_end - proj_end).norm();

    let delta_t = (t_end - t_start).abs();
    let delta_r = (r_end - r_start).abs();

    // Perpendicular to axis (same t, different r)
    if delta_t < tol.linear && delta_r > tol.linear {
        return RevolveSurfaceType::Plane { height: t_start };
    }

    // Parallel to axis (same r, different t)
    if delta_r < tol.linear && delta_t > tol.linear {
        return RevolveSurfaceType::Cylinder { radius: r_start };
    }

    // Angled: compute cone apex and half-angle
    let s_apex = -r_start / (r_end - r_start);
    let t_apex = t_start + s_apex * (t_end - t_start);
    let apex = *axis_origin + t_apex * axis;
    let half_angle = (delta_r / delta_t).atan();

    RevolveSurfaceType::Cone { apex, half_angle }
}

#[allow(clippy::too_many_arguments)]
fn build_full_cylinder_face<F>(
    topo: &mut Topology,
    geom: &mut GeometryStore,
    axis_origin: &Point3,
    axis: &Vec3,
    radius: f64,
    v_bot: &VertexId,
    v_top: &VertexId,
    outward: bool,
    he_map: &mut HashMap<([i64; 3], [i64; 3]), HalfEdgeId>,
    quantize_pt: F,
) -> vcad_kernel_topo::FaceId
where
    F: Fn(Point3) -> [i64; 3],
{
    let cyl_surface = CylinderSurface::with_axis(*axis_origin, *axis, radius);
    let surf_idx = geom.add_surface(Box::new(cyl_surface));

    // Full revolution: degenerate quad with seam
    // Two half-edges for seam (vertical), two for circles (degenerate)
    let he_bot = topo.add_half_edge(*v_bot); // bottom circle (degenerate)
    let he_seam_up = topo.add_half_edge(*v_bot); // seam going up
    let he_top = topo.add_half_edge(*v_top); // top circle (degenerate)
    let he_seam_down = topo.add_half_edge(*v_top); // seam going down

    let loop_id = topo.add_loop(&[he_bot, he_seam_up, he_top, he_seam_down]);
    // The cylinder surface normal points radially outward; an inner wall
    // (e.g. the bore of a revolved washer) faces the axis instead.
    let orientation = if outward {
        Orientation::Forward
    } else {
        Orientation::Reversed
    };
    let face_id = topo.add_face(loop_id, surf_idx, orientation);

    // Record in he_map
    for &he_id in &[he_bot, he_seam_up, he_top, he_seam_down] {
        let he = &topo.half_edges[he_id];
        let origin = topo.vertices[he.origin].point;
        if let Some(next) = he.next {
            let dest = topo.vertices[topo.half_edges[next].origin].point;
            he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
        }
    }

    face_id
}

/// Annular (or disk) planar face for a full revolution of a segment
/// perpendicular to the axis. The outer boundary is a degenerate
/// single-vertex circle loop, matching the primitive cylinder-cap
/// representation the tessellator and boolean pipeline special-case; the
/// inner hole is a densely sampled polygon, matching how boolean splits
/// encode circular holes (`split_planar_face_by_circle`) — degenerate
/// inner loops are filtered out by the hole-aware tessellation.
#[allow(clippy::too_many_arguments)]
fn build_full_annular_face(
    topo: &mut Topology,
    geom: &mut GeometryStore,
    vertex_cache: &mut HashMap<[i64; 3], VertexId>,
    axis_origin: &Point3,
    axis: &Vec3,
    rt_a: (f64, f64),
    rt_b: (f64, f64),
    v_a: &VertexId,
    v_b: &VertexId,
    axis_sign: f64,
    quantize_pt: impl Fn(Point3) -> [i64; 3],
) -> vcad_kernel_topo::FaceId {
    let t_plane = rt_a.1;
    let center = *axis_origin + t_plane * axis;
    let plane = Plane::from_normal(center, *axis * axis_sign);
    let surf_idx = geom.add_surface(Box::new(plane));

    // Outer boundary at the larger radius, inner hole at the smaller.
    let (v_outer, v_inner) = if rt_a.0 >= rt_b.0 {
        (v_a, v_b)
    } else {
        (v_b, v_a)
    };

    let he_outer = topo.add_half_edge(*v_outer);
    let outer_loop = topo.add_loop(&[he_outer]);
    let face_id = topo.add_face(outer_loop, surf_idx, Orientation::Forward);

    // Dense inner circle through the inner seam vertex.
    const INNER_SEGMENTS: usize = 32;
    let p_inner = topo.vertices[*v_inner].point;
    let mut inner_hes = Vec::with_capacity(INNER_SEGMENTS);
    for k in 0..INNER_SEGMENTS {
        let theta = 2.0 * PI * k as f64 / INNER_SEGMENTS as f64;
        let p = rotate_point(&p_inner, axis_origin, axis, theta);
        let key = quantize_pt(p);
        let vid = *vertex_cache
            .entry(key)
            .or_insert_with(|| topo.add_vertex(p));
        inner_hes.push(topo.add_half_edge(vid));
    }
    let inner_loop = topo.add_loop(&inner_hes);
    topo.add_inner_loop(face_id, inner_loop);

    face_id
}

/// One angular facet of a swept profile segment: a planar quad between
/// θ0 and θ1. Winding is chosen from the profile's (r, t) orientation so
/// the quad normal points out of the solid: for a CCW profile the outward
/// direction is (dt·radial − dr·axis), which corresponds to walking the
/// quad start-edge before the θ1 arc.
#[allow(clippy::too_many_arguments)]
fn build_facet_quad<F>(
    topo: &mut Topology,
    geom: &mut GeometryStore,
    vertex_cache: &mut HashMap<[i64; 3], VertexId>,
    axis_origin: &Point3,
    axis: &Vec3,
    p_start: &Point3,
    p_end: &Point3,
    theta0: f64,
    theta1: f64,
    ccw: bool,
    he_map: &mut HashMap<([i64; 3], [i64; 3]), HalfEdgeId>,
    quantize_pt: F,
) -> vcad_kernel_topo::FaceId
where
    F: Fn(Point3) -> [i64; 3],
{
    let s0 = rotate_point(p_start, axis_origin, axis, theta0);
    let s1 = rotate_point(p_start, axis_origin, axis, theta1);
    let e0 = rotate_point(p_end, axis_origin, axis, theta0);
    let e1 = rotate_point(p_end, axis_origin, axis, theta1);

    let corners: [Point3; 4] = if ccw {
        [s0, s1, e1, e0]
    } else {
        [s0, e0, e1, s1]
    };

    let plane = Plane::new(corners[0], corners[1] - corners[0], corners[3] - corners[0]);
    let surf_idx = geom.add_surface(Box::new(plane));

    let quantize = &quantize_pt;
    let vids: Vec<VertexId> = corners
        .iter()
        .map(|p| {
            let key = quantize(*p);
            *vertex_cache
                .entry(key)
                .or_insert_with(|| topo.add_vertex(*p))
        })
        .collect();

    let hes: Vec<HalfEdgeId> = vids.iter().map(|&v| topo.add_half_edge(v)).collect();
    let loop_id = topo.add_loop(&hes);
    let face_id = topo.add_face(loop_id, surf_idx, Orientation::Forward);

    for &he_id in &hes {
        let he = &topo.half_edges[he_id];
        let origin = topo.vertices[he.origin].point;
        if let Some(next) = he.next {
            let dest = topo.vertices[topo.half_edges[next].origin].point;
            he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
        }
    }

    face_id
}

fn build_side_face<F>(
    topo: &mut Topology,
    geom: &mut GeometryStore,
    verts: &[VertexId],
    he_map: &mut HashMap<([i64; 3], [i64; 3]), HalfEdgeId>,
    quantize_pt: F,
    reversed: bool,
) -> vcad_kernel_topo::FaceId
where
    F: Fn(Point3) -> [i64; 3],
{
    let positions: Vec<Point3> = verts.iter().map(|&v| topo.vertices[v].point).collect();
    let n = positions.len();

    // Create plane from vertices
    let plane = if n >= 3 {
        Plane::new(
            positions[0],
            positions[1] - positions[0],
            positions[n - 1] - positions[0],
        )
    } else {
        Plane::from_normal(positions[0], Vec3::z())
    };
    let surf_idx = geom.add_surface(Box::new(plane));

    // Create half-edges
    let ordered_verts: Vec<VertexId> = if reversed {
        verts.iter().rev().copied().collect()
    } else {
        verts.to_vec()
    };

    let hes: Vec<HalfEdgeId> = ordered_verts
        .iter()
        .map(|&v| topo.add_half_edge(v))
        .collect();
    let loop_id = topo.add_loop(&hes);
    let face_id = topo.add_face(loop_id, surf_idx, Orientation::Forward);

    for &he_id in &hes {
        let he = &topo.half_edges[he_id];
        let origin = topo.vertices[he.origin].point;
        if let Some(next) = he.next {
            let dest = topo.vertices[topo.half_edges[next].origin].point;
            he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
        }
    }

    face_id
}

fn pair_twin_half_edges(topo: &mut Topology, he_map: &HashMap<([i64; 3], [i64; 3]), HalfEdgeId>) {
    let mut paired = std::collections::HashSet::new();

    for (&(origin_key, dest_key), &he_id) in he_map {
        if paired.contains(&(dest_key, origin_key)) {
            continue;
        }

        if let Some(&twin_id) = he_map.get(&(dest_key, origin_key)) {
            if topo.half_edges[he_id].twin.is_none() && topo.half_edges[twin_id].twin.is_none() {
                topo.add_edge(he_id, twin_id);
                paired.insert((origin_key, dest_key));
            }
        }
    }
}

fn point_to_line_distance(point: &Point3, line_origin: &Point3, line_dir: &Vec3) -> f64 {
    let v = point - line_origin;
    let proj = v.dot(line_dir) * line_dir;
    (v - proj).norm()
}

fn rotate_point(point: &Point3, axis_origin: &Point3, axis: &Vec3, angle: f64) -> Point3 {
    let v = point - axis_origin;

    let (sin_a, cos_a) = angle.sin_cos();
    let one_minus_cos = 1.0 - cos_a;

    let (x, y, z) = (axis.x, axis.y, axis.z);

    // Rodrigues' rotation formula
    let rotated = Vec3::new(
        (cos_a + one_minus_cos * x * x) * v.x
            + (one_minus_cos * x * y - sin_a * z) * v.y
            + (one_minus_cos * x * z + sin_a * y) * v.z,
        (one_minus_cos * x * y + sin_a * z) * v.x
            + (cos_a + one_minus_cos * y * y) * v.y
            + (one_minus_cos * y * z - sin_a * x) * v.z,
        (one_minus_cos * x * z - sin_a * y) * v.x
            + (one_minus_cos * y * z + sin_a * x) * v.y
            + (cos_a + one_minus_cos * z * z) * v.z,
    );

    *axis_origin + rotated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_revolve_rectangle_full() {
        // Rectangle offset from Y-axis, revolve full 360° → hollow cylinder
        let profile = SketchProfile::rectangle(
            Point3::new(5.0, 0.0, 0.0),
            Vec3::x(),
            Vec3::z(),
            3.0,  // radial extent: 5 to 8
            10.0, // height
        );

        let solid = revolve(&profile, Point3::origin(), Vec3::z(), 2.0 * PI).unwrap();

        // 4 segments → 4 revolution faces
        // Full revolution: no side faces
        assert_eq!(solid.topology.faces.len(), 4);
    }

    #[test]
    fn test_revolve_rectangle_90_degrees() {
        let profile =
            SketchProfile::rectangle(Point3::new(5.0, 0.0, 0.0), Vec3::x(), Vec3::z(), 3.0, 10.0);

        let solid = revolve(&profile, Point3::origin(), Vec3::z(), PI / 2.0).unwrap();

        // Partial revolutions sweep each profile segment into angular
        // facets (90° → 8 steps at the default 32-per-turn density):
        // 4 segments × 8 facets + 2 side caps.
        assert_eq!(solid.topology.faces.len(), 4 * 8 + 2);

        // 4 profile vertices × 9 rings.
        assert_eq!(solid.topology.vertices.len(), 4 * 9);
    }

    #[test]
    fn test_revolve_triangle_to_cone() {
        // Profile in XZ plane, offset from Z-axis
        let profile =
            SketchProfile::rectangle(Point3::new(2.0, 0.0, 0.0), Vec3::x(), Vec3::z(), 5.0, 10.0);

        let solid = revolve(&profile, Point3::origin(), Vec3::z(), 2.0 * PI).unwrap();

        // Should have 4 faces (one per profile segment)
        assert_eq!(solid.topology.faces.len(), 4);
    }

    #[test]
    fn test_revolve_zero_axis_error() {
        let profile =
            SketchProfile::rectangle(Point3::new(5.0, 0.0, 0.0), Vec3::x(), Vec3::z(), 5.0, 10.0);

        let result = revolve(&profile, Point3::origin(), Vec3::zeros(), PI);
        assert!(matches!(result, Err(SketchError::ZeroAxis)));
    }

    #[test]
    fn test_revolve_invalid_angle_error() {
        let profile =
            SketchProfile::rectangle(Point3::new(5.0, 0.0, 0.0), Vec3::x(), Vec3::z(), 3.0, 10.0);

        // Negative angle
        let result = revolve(&profile, Point3::origin(), Vec3::z(), -1.0);
        assert!(matches!(result, Err(SketchError::InvalidAngle(_))));

        // Zero angle
        let result = revolve(&profile, Point3::origin(), Vec3::z(), 0.0);
        assert!(matches!(result, Err(SketchError::InvalidAngle(_))));

        // Angle > 2π
        let result = revolve(&profile, Point3::origin(), Vec3::z(), 3.0 * PI);
        assert!(matches!(result, Err(SketchError::InvalidAngle(_))));
    }

    #[test]
    fn test_revolve_arc_not_supported() {
        let profile = SketchProfile::circle(Point3::new(10.0, 0.0, 0.0), Vec3::x(), 3.0, 4);

        let result = revolve(&profile, Point3::origin(), Vec3::z(), PI);
        assert!(matches!(result, Err(SketchError::ArcNotSupported)));
    }

    #[test]
    fn test_revolve_axis_intersection_error() {
        // Profile with a vertex on the Z-axis (x=0, y=0)
        // Rectangle at origin in XZ plane, one corner at (0,0,0) which is on Z-axis
        let profile = SketchProfile::rectangle(
            Point3::origin(), // Origin at (0, 0, 0) which is on Z-axis
            Vec3::x(),
            Vec3::z(),
            5.0,
            5.0,
        );

        let result = revolve(&profile, Point3::origin(), Vec3::z(), PI);
        assert!(matches!(result, Err(SketchError::AxisIntersection)));
    }

    #[test]
    fn test_revolve_90_degrees_volume() {
        // Rectangle profile: inner radius 5, outer radius 8, height 10
        // Quarter-annulus volume = π * (R² - r²) * h / 4 = π * (64 - 25) * 10 / 4 ≈ 306.3
        let profile =
            SketchProfile::rectangle(Point3::new(5.0, 0.0, 0.0), Vec3::x(), Vec3::z(), 3.0, 10.0);

        let solid = revolve(&profile, Point3::origin(), Vec3::z(), PI / 2.0).unwrap();

        // All half-edges should be paired
        let unpaired = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .count();
        assert_eq!(unpaired, 0, "all half-edges should be paired");

        let mesh = vcad_kernel_tessellate::tessellate_brep(&solid, 64);
        let vol = compute_mesh_volume(&mesh);

        let expected = PI * (8.0 * 8.0 - 5.0 * 5.0) * 10.0 / 4.0;
        // Using planar approximation for curved faces
        // Planar quads for 90° arcs underestimate significantly (~36% error)
        // Just verify we get a reasonable positive volume
        assert!(
            vol > expected * 0.5 && vol < expected * 1.2,
            "expected volume ~{expected:.1} (±50%), got {vol:.1}"
        );
    }

    #[test]
    fn test_classify_parallel_line() {
        let p_start = Point3::new(5.0, 0.0, 0.0);
        let p_end = Point3::new(5.0, 0.0, 10.0);
        let axis_origin = Point3::origin();
        let axis = Vec3::z();

        let result = classify_line_segment(&p_start, &p_end, &axis_origin, &axis);
        match result {
            RevolveSurfaceType::Cylinder { radius } => {
                assert!((radius - 5.0).abs() < 1e-10);
            }
            _ => panic!("expected cylinder"),
        }
    }

    #[test]
    fn test_classify_perpendicular_line() {
        let p_start = Point3::new(5.0, 0.0, 5.0);
        let p_end = Point3::new(8.0, 0.0, 5.0);
        let axis_origin = Point3::origin();
        let axis = Vec3::z();

        let result = classify_line_segment(&p_start, &p_end, &axis_origin, &axis);
        match result {
            RevolveSurfaceType::Plane { height } => {
                assert!((height - 5.0).abs() < 1e-10);
            }
            _ => panic!("expected plane"),
        }
    }

    #[test]
    fn test_classify_angled_line() {
        let p_start = Point3::new(5.0, 0.0, 0.0);
        let p_end = Point3::new(10.0, 0.0, 10.0);
        let axis_origin = Point3::origin();
        let axis = Vec3::z();

        let result = classify_line_segment(&p_start, &p_end, &axis_origin, &axis);
        match result {
            RevolveSurfaceType::Cone { .. } => {
                // Expected
            }
            _ => panic!("expected cone"),
        }
    }

    fn compute_mesh_volume(mesh: &vcad_kernel_tessellate::TriangleMesh) -> f64 {
        let verts = &mesh.vertices;
        let indices = &mesh.indices;
        let mut vol = 0.0;
        for tri in indices.chunks(3) {
            let (i0, i1, i2) = (
                tri[0] as usize * 3,
                tri[1] as usize * 3,
                tri[2] as usize * 3,
            );
            let v0 = [verts[i0] as f64, verts[i0 + 1] as f64, verts[i0 + 2] as f64];
            let v1 = [verts[i1] as f64, verts[i1 + 1] as f64, verts[i1 + 2] as f64];
            let v2 = [verts[i2] as f64, verts[i2 + 1] as f64, verts[i2 + 2] as f64];
            vol += v0[0] * (v1[1] * v2[2] - v2[1] * v1[2])
                - v1[0] * (v0[1] * v2[2] - v2[1] * v0[2])
                + v2[0] * (v0[1] * v1[2] - v1[1] * v0[2]);
        }
        (vol / 6.0).abs()
    }
}
