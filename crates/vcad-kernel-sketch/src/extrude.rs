//! Extrude operation: create a solid by sweeping a profile along a direction.

use std::collections::HashMap;
use std::f64::consts::PI;

use vcad_kernel_geom::{BilinearSurface, Circle3d, CylinderSurface, GeometryStore, Line3d, Plane};
use vcad_kernel_math::{Dir3, Point2, Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{HalfEdgeId, Orientation, ShellType, Topology, VertexId};

use crate::{SketchError, SketchProfile, SketchSegment};

/// Options for the extrude operation.
#[derive(Debug, Clone)]
pub struct ExtrudeOptions {
    /// Total twist angle along the extrusion (in radians). Default: 0.0
    pub twist_angle: f64,
    /// Scale factor at the end of the extrusion. Default: 1.0
    pub scale_end: f64,
    /// Number of segments per arc in the profile. Default: 8.
    pub arc_segments: u32,
}

impl Default for ExtrudeOptions {
    fn default() -> Self {
        Self {
            twist_angle: 0.0,
            scale_end: 1.0,
            arc_segments: 8,
        }
    }
}

/// Extrude a closed profile along a direction to create a B-rep solid.
///
/// # Arguments
///
/// * `profile` - The closed 2D profile to extrude
/// * `direction` - The extrusion direction vector (magnitude = distance)
///
/// # Returns
///
/// A B-rep solid with:
/// - One lateral face per segment (planar for lines, cylindrical for arcs)
/// - Two cap faces (bottom at profile plane, top translated by direction)
///
/// # Errors
///
/// Returns an error if the direction vector is zero.
///
/// # Example
///
/// ```
/// use vcad_kernel_sketch::{SketchProfile, extrude};
/// use vcad_kernel_math::{Point3, Vec3};
///
/// let profile = SketchProfile::rectangle(
///     Point3::origin(),
///     Vec3::x(),
///     Vec3::y(),
///     10.0, 5.0,
/// );
/// let solid = extrude(&profile, Vec3::new(0.0, 0.0, 20.0)).unwrap();
/// // Creates a 10x5x20 box
/// ```
pub fn extrude(profile: &SketchProfile, direction: Vec3) -> Result<BRepSolid, SketchError> {
    let dir_len = direction.norm();
    if dir_len < 1e-12 {
        return Err(SketchError::ZeroExtrusion);
    }

    // Analytic fast path: a profile that is a full circle extruded along
    // its plane normal becomes a true right circular cylinder — one
    // CylinderSurface lateral face plus two planar disk caps, matching
    // vcad_kernel_primitives::make_cylinder. This gives pixel-perfect
    // curved surfaces instead of the N-gon approximation that a
    // tessellated profile would produce.
    if let Some(fc) = detect_axial_full_circle(profile, direction) {
        return Ok(extrude_full_circle_analytic(profile, fc, direction));
    }

    // Arcs that aren't a full analytic circle are extruded into analytic
    // cylindrical lateral faces (one CylinderSurface per arc). The cap
    // outlines get densified so the 3D cap polygons look like arcs instead
    // of coarse chord polygons, and each cap vertex is shared with the
    // lateral cylindrical face's rich outer loop so the half-edge topology
    // twins correctly. Only arc-bearing profiles take the analytic path;
    // pure polylines keep the simpler original build below.
    if !profile.is_line_only() {
        let arc_segs = ExtrudeOptions::default().arc_segments as usize;
        return Ok(extrude_with_arcs(profile, direction, arc_segs));
    }

    let mut topo = Topology::new();
    let mut geom = GeometryStore::new();

    let n_segments = profile.segments.len();

    // Vertex cache: quantized position -> VertexId. The 1e-9 grid (1 pm in
    // mm units) is deliberately three orders finer than the sew tolerance
    // (1e-6): this cache only unifies float-identical endpoints shared by
    // consecutive segments and by the cap/lateral construction — proximity
    // welding of genuinely distinct vertices is sew's job, and a coarser
    // grid here would alias real geometry instead of preventing it.
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

    // Create bottom and top vertices for each segment endpoint
    let mut bottom_verts: Vec<VertexId> = Vec::with_capacity(n_segments);
    let mut top_verts: Vec<VertexId> = Vec::with_capacity(n_segments);

    for seg in &profile.segments {
        let start_2d = seg.start();
        let start_3d = profile.to_3d(start_2d);
        let top_3d = start_3d + direction;

        let bot_v = get_or_create_vertex(&mut vertex_cache, &mut topo, start_3d);
        let top_v = get_or_create_vertex(&mut vertex_cache, &mut topo, top_3d);

        bottom_verts.push(bot_v);
        top_verts.push(top_v);
    }

    let mut all_faces = Vec::new();

    // Track half-edges for twin pairing: (origin_key, dest_key) -> HalfEdgeId
    let mut he_map: HashMap<([i64; 3], [i64; 3]), HalfEdgeId> = HashMap::new();

    // Build lateral faces (one per segment)
    // The profile goes CCW when viewed from +normal direction.
    // For each segment from point i to point i+1:
    //   - bottom edge: bot[i] -> bot[i+1]
    //   - top edge: top[i+1] -> top[i]  (reversed)
    //   - left edge: bot[i] -> top[i]
    //   - right edge: top[i+1] -> bot[i+1]
    //
    // For outward-pointing normals on the lateral face:
    // Winding should be: bot[i] -> bot[i+1] -> top[i+1] -> top[i]
    // This creates a face whose normal points away from the solid interior.

    for (i, seg) in profile.segments.iter().enumerate() {
        let next_i = (i + 1) % n_segments;

        let bot_i = bottom_verts[i];
        let bot_next = bottom_verts[next_i];
        let top_i = top_verts[i];
        let top_next = top_verts[next_i];

        let bot_i_pos = topo.vertices[bot_i].point;
        let bot_next_pos = topo.vertices[bot_next].point;
        let top_i_pos = topo.vertices[top_i].point;
        let top_next_pos = topo.vertices[top_next].point;

        // Create lateral face with winding: bot_i -> bot_next -> top_next -> top_i
        let (face_id, face_hes) = match seg {
            SketchSegment::Line { .. } => build_planar_lateral_face(
                &mut topo,
                &mut geom,
                bot_i,
                bot_next,
                top_next,
                top_i,
                bot_i_pos,
                bot_next_pos,
                top_next_pos,
                top_i_pos,
            ),
            SketchSegment::Arc { center, ccw, .. } => build_cylindrical_lateral_face(
                &mut topo, &mut geom, profile, bot_i, bot_next, top_next, top_i, *center, *ccw,
                &direction,
            ),
        };

        all_faces.push(face_id);

        // Record half-edges for twin pairing
        for he_id in face_hes {
            let he = &topo.half_edges[he_id];
            let origin = topo.vertices[he.origin].point;
            let next = he.next.unwrap();
            let dest = topo.vertices[topo.half_edges[next].origin].point;
            he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
        }
    }

    // Build bottom cap face
    // The profile vertices go CCW when viewed from +normal.
    // Bottom face normal should point in -direction (opposite to extrusion).
    // For -direction normal with CCW vertices, we need CW winding (reversed).
    let bot_cap_face_id = build_cap_face(
        &mut topo,
        &mut geom,
        &bottom_verts,
        &-*profile.normal.as_ref(),
        true, // reversed winding for outward (-direction) normal
        &mut he_map,
        quantize_pt,
    );
    all_faces.push(bot_cap_face_id);

    // Build top cap face
    // Top face normal should point in +direction.
    // Profile vertices map to top with same order, so CCW gives +direction normal.
    let top_cap_face_id = build_cap_face(
        &mut topo,
        &mut geom,
        &top_verts,
        profile.normal.as_ref(),
        false, // forward winding for outward (+direction) normal
        &mut he_map,
        quantize_pt,
    );
    all_faces.push(top_cap_face_id);

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

/// A full-circle profile detected for the analytic-cylinder extrude path.
struct FullCircle {
    center: Point2,
    radius: f64,
}

/// Detect a profile that is a closed full circle, being extruded along its
/// plane normal. Returns `None` for any profile containing a non-arc
/// segment, arcs with disagreeing centers/radii, a signed-angle sum that
/// isn't ±2π, or an extrusion direction that isn't (anti-)parallel to the
/// profile normal.
fn detect_axial_full_circle(profile: &SketchProfile, direction: Vec3) -> Option<FullCircle> {
    if profile.segments.is_empty() {
        return None;
    }

    // Axial: direction must be parallel to the profile normal (either sign).
    let dir_unit = direction.normalize();
    let axial_dot = dir_unit.dot(profile.normal.as_ref()).abs();
    if axial_dot < 1.0 - 1e-6 {
        return None;
    }

    // Reference center/radius from the first segment.
    let (ref_center, ref_radius) = match &profile.segments[0] {
        SketchSegment::Arc { start, center, .. } => (*center, (start - center).norm()),
        SketchSegment::Line { .. } => return None,
    };
    if ref_radius < 1e-9 {
        return None;
    }

    let center_tol = ref_radius * 1e-6 + 1e-9;
    let radius_tol = ref_radius * 1e-6 + 1e-9;
    let mut total_angle = 0.0;

    for seg in &profile.segments {
        let (start, end, center, ccw) = match seg {
            SketchSegment::Arc {
                start,
                end,
                center,
                ccw,
            } => (*start, *end, *center, *ccw),
            SketchSegment::Line { .. } => return None,
        };
        if (center - ref_center).norm() > center_tol {
            return None;
        }
        let r_start = (start - center).norm();
        let r_end = (end - center).norm();
        if (r_start - ref_radius).abs() > radius_tol || (r_end - ref_radius).abs() > radius_tol {
            return None;
        }

        let d_start = start - center;
        let d_end = end - center;
        let start_angle = d_start.y.atan2(d_start.x);
        let end_angle = d_end.y.atan2(d_end.x);
        let mut angle = end_angle - start_angle;
        if ccw {
            if angle <= 0.0 {
                angle += 2.0 * PI;
            }
        } else if angle >= 0.0 {
            angle -= 2.0 * PI;
        }
        total_angle += angle;
    }

    if (total_angle.abs() - 2.0 * PI).abs() > 1e-6 {
        return None;
    }

    Some(FullCircle {
        center: ref_center,
        radius: ref_radius,
    })
}

/// Build an analytic-surface BRep for a full-circle profile extruded along
/// its normal: one `CylinderSurface` lateral face with a seam edge, and two
/// planar disk caps. Mirrors `vcad_kernel_primitives::make_cylinder`.
fn extrude_full_circle_analytic(
    profile: &SketchProfile,
    fc: FullCircle,
    direction: Vec3,
) -> BRepSolid {
    let mut topo = Topology::new();
    let mut geom = GeometryStore::new();

    let axis = Dir3::new_normalize(direction);
    let ref_dir = profile.x_dir; // seam direction lies along profile +X
    let bot_center = profile.to_3d(fc.center);
    let top_center = bot_center + direction;
    let seam_offset = *ref_dir.as_ref() * fc.radius;

    // Seam vertices (at u=0 on the cylinder's parameterization).
    let v_bot = topo.add_vertex(bot_center + seam_offset);
    let v_top = topo.add_vertex(top_center + seam_offset);

    // Surfaces.
    let cyl = CylinderSurface {
        center: bot_center,
        axis,
        ref_dir,
        radius: fc.radius,
    };
    let cyl_idx = geom.add_surface(Box::new(cyl));

    // Bottom cap: outward normal = -axis. Top cap: outward normal = +axis.
    let bot_plane = Plane::from_normal(bot_center, -*axis.as_ref());
    let bot_idx = geom.add_surface(Box::new(bot_plane));
    let top_plane = Plane::from_normal(top_center, *axis.as_ref());
    let top_idx = geom.add_surface(Box::new(top_plane));

    // Curves for provenance (bottom circle, top circle, seam line).
    geom.add_curve_3d(Box::new(Circle3d::with_normal(
        bot_center,
        fc.radius,
        *axis.as_ref(),
    )));
    geom.add_curve_3d(Box::new(Circle3d::with_normal(
        top_center,
        fc.radius,
        *axis.as_ref(),
    )));
    geom.add_curve_3d(Box::new(Line3d::from_points(
        bot_center + seam_offset,
        top_center + seam_offset,
    )));

    // Lateral face: single loop with a seam.
    //   he_bot_lat : v_bot → v_bot (bottom circle, full sweep)
    //   he_seam_up : v_bot → v_top (seam)
    //   he_top_lat : v_top → v_top (top circle, reversed sweep)
    //   he_seam_dn : v_top → v_bot (seam return)
    let he_bot_lat = topo.add_half_edge(v_bot);
    let he_seam_up = topo.add_half_edge(v_bot);
    let he_top_lat = topo.add_half_edge(v_top);
    let he_seam_dn = topo.add_half_edge(v_top);
    let lat_loop = topo.add_loop(&[he_bot_lat, he_seam_up, he_top_lat, he_seam_dn]);
    let lat_face = topo.add_face(lat_loop, cyl_idx, Orientation::Forward);

    // Caps: each is a single degenerate half-edge loop. The tessellator
    // detects loop_len<=1 on a Plane face and samples the disk from the
    // surface + loop vertex radius.
    let he_bot_cap = topo.add_half_edge(v_bot);
    let bot_loop = topo.add_loop(&[he_bot_cap]);
    let bot_face = topo.add_face(bot_loop, bot_idx, Orientation::Forward);

    let he_top_cap = topo.add_half_edge(v_top);
    let top_loop = topo.add_loop(&[he_top_cap]);
    let top_face = topo.add_face(top_loop, top_idx, Orientation::Forward);

    // Twin pairing: each circle edge has a lateral side and a cap side;
    // the seam has its two directions.
    topo.add_edge(he_bot_lat, he_bot_cap);
    topo.add_edge(he_top_lat, he_top_cap);
    topo.add_edge(he_seam_up, he_seam_dn);

    let shell = topo.add_shell(vec![lat_face, bot_face, top_face], ShellType::Outer);
    let solid_id = topo.add_solid(shell);

    BRepSolid {
        topology: topo,
        geometry: geom,
        solid_id,
    }
}

/// Extrude a profile whose segments include non-full-circle arcs.
///
/// Each arc segment becomes one analytic `CylinderSurface` lateral face;
/// each line segment becomes one `Plane` lateral face. The outer loop of
/// every cylindrical face includes `arc_segs + 1` vertices along each of
/// its top and bottom arc boundaries so the cap polygons stay faithful to
/// the original profile and share their boundary vertices with the
/// adjacent lateral faces (required for half-edge twin pairing).
///
/// This path is taken in preference to the generic polygonal builder
/// whenever a profile contains any arc; it preserves the analytic surface
/// type end-to-end (tessellator, booleans, STEP export, fillet) so
/// downstream ops don't have to guess that a curved wall lurks inside a
/// strip of thin near-coplanar quads.
fn extrude_with_arcs(profile: &SketchProfile, direction: Vec3, arc_segs: usize) -> BRepSolid {
    let arc_segs = arc_segs.max(2);

    let mut topo = Topology::new();
    let mut geom = GeometryStore::new();

    let quantize_pt = |p: Point3| -> [i64; 3] {
        [
            (p.x * 1e9).round() as i64,
            (p.y * 1e9).round() as i64,
            (p.z * 1e9).round() as i64,
        ]
    };

    let mut vertex_cache: HashMap<[i64; 3], VertexId> = HashMap::new();
    let get_or_create =
        |cache: &mut HashMap<[i64; 3], VertexId>, topo: &mut Topology, pos: Point3| -> VertexId {
            let key = quantize_pt(pos);
            *cache.entry(key).or_insert_with(|| topo.add_vertex(pos))
        };

    // Walk each segment, densifying arcs. Every tessellation point becomes a
    // shared vertex pair (bot + top); lateral faces and cap faces both
    // reference these so the half-edge twin pairing works. Each segment
    // owns a contiguous slice of the chain, with its first vertex shared
    // with the previous segment's last vertex.
    let mut bot_chain: Vec<VertexId> = Vec::new();
    let mut top_chain: Vec<VertexId> = Vec::new();
    // Inclusive ranges: seg `i` spans chain[seg_first[i]..=seg_last[i]].
    let mut seg_first: Vec<usize> = Vec::with_capacity(profile.segments.len());
    let mut seg_last: Vec<usize> = Vec::with_capacity(profile.segments.len());

    for (idx, seg) in profile.segments.iter().enumerate() {
        let tess_points_2d: Vec<Point2> = match seg {
            SketchSegment::Line { start, end } => {
                if idx == 0 {
                    vec![*start, *end]
                } else {
                    vec![*end]
                }
            }
            SketchSegment::Arc {
                start,
                end,
                center,
                ccw,
            } => {
                let mut pts = sample_arc_2d(*start, *end, *center, *ccw, arc_segs);
                if idx != 0 {
                    pts.remove(0);
                }
                pts
            }
        };

        // First index of this segment: either 0 (for seg 0) or the last
        // index of the previous segment (shared endpoint).
        let first = if idx == 0 {
            0
        } else {
            *seg_last.last().unwrap()
        };
        seg_first.push(first);

        for p2 in tess_points_2d {
            let p3 = profile.to_3d(p2);
            let bot_v = get_or_create(&mut vertex_cache, &mut topo, p3);
            let top_v = get_or_create(&mut vertex_cache, &mut topo, p3 + direction);
            bot_chain.push(bot_v);
            top_chain.push(top_v);
        }

        seg_last.push(bot_chain.len() - 1);
    }

    let mut all_faces = Vec::new();
    let mut he_map: HashMap<([i64; 3], [i64; 3]), HalfEdgeId> = HashMap::new();

    // Build one lateral face per original segment.
    for (idx, seg) in profile.segments.iter().enumerate() {
        let start_idx = seg_first[idx];
        let end_idx = seg_last[idx];
        let slice_len = end_idx - start_idx + 1;
        let bot_slice: Vec<VertexId> = (0..slice_len).map(|k| bot_chain[start_idx + k]).collect();
        let top_slice: Vec<VertexId> = (0..slice_len).map(|k| top_chain[start_idx + k]).collect();

        // Outer loop: bot_0 → ... → bot_k → top_k → top_{k-1} → ... → top_0.
        // Each pair of consecutive vertices contributes one half-edge.
        let mut loop_hes: Vec<HalfEdgeId> = Vec::with_capacity(2 * slice_len);
        for w in bot_slice.windows(2) {
            loop_hes.push(topo.add_half_edge(w[0]));
            let _ = w[1];
        }
        loop_hes.push(topo.add_half_edge(bot_slice[slice_len - 1])); // right edge start
        for w in top_slice.windows(2).rev() {
            loop_hes.push(topo.add_half_edge(w[1]));
            let _ = w[0];
        }
        loop_hes.push(topo.add_half_edge(top_slice[0])); // left edge start

        // Surface
        let surf_idx = match seg {
            SketchSegment::Line { .. } => {
                let p0 = topo.vertices[bot_slice[0]].point;
                let p1 = topo.vertices[bot_slice[slice_len - 1]].point;
                let x_dir = p1 - p0;
                let y_dir = direction;
                if x_dir.norm() > 1e-12 && y_dir.cross(x_dir).norm() > 1e-12 {
                    geom.add_surface(Box::new(Plane::new(p0, x_dir, y_dir)))
                } else {
                    geom.add_surface(Box::new(Plane::from_normal(p0, y_dir.cross(x_dir))))
                }
            }
            SketchSegment::Arc { start, center, .. } => {
                let center_3d = profile.to_3d(*center);
                let radius = (*start - *center).norm();
                let axis = Dir3::new_normalize(direction);
                let start_3d = topo.vertices[bot_slice[0]].point;
                let radial = start_3d - center_3d;
                let ref_dir =
                    Dir3::new_normalize(radial - radial.dot(axis.as_ref()) * *axis.as_ref());
                let cyl = CylinderSurface {
                    center: center_3d,
                    axis,
                    ref_dir,
                    radius,
                };
                geom.add_surface(Box::new(cyl))
            }
        };

        let loop_id = topo.add_loop(&loop_hes);
        // A clockwise arc in a CCW profile is concave: the solid material
        // lies *outside* its cylinder, so the face normal (which
        // CylinderSurface always evaluates radially outward) must be
        // flipped for the face to point out of the solid.
        let orientation = match seg {
            SketchSegment::Arc { ccw: false, .. } => Orientation::Reversed,
            _ => Orientation::Forward,
        };
        let face_id = topo.add_face(loop_id, surf_idx, orientation);
        all_faces.push(face_id);

        // Record half-edges for twin pairing
        for &he_id in &loop_hes {
            let he = &topo.half_edges[he_id];
            let origin = topo.vertices[he.origin].point;
            let next = he.next.unwrap();
            let dest = topo.vertices[topo.half_edges[next].origin].point;
            he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
        }
    }

    // Build cap faces. Drop the final duplicate (chains close with
    // bot_chain[last] == bot_chain[first]) so caps become a single circular
    // loop.
    let n_chain = bot_chain.len();
    let last_is_dup = bot_chain[n_chain - 1] == bot_chain[0];
    let chain_len = if last_is_dup { n_chain - 1 } else { n_chain };
    let cap_bot: Vec<VertexId> = (0..chain_len).map(|i| bot_chain[i]).collect();
    let cap_top: Vec<VertexId> = (0..chain_len).map(|i| top_chain[i]).collect();

    let bot_cap_face_id = build_cap_face(
        &mut topo,
        &mut geom,
        &cap_bot,
        &-*profile.normal.as_ref(),
        true,
        &mut he_map,
        quantize_pt,
    );
    all_faces.push(bot_cap_face_id);

    let top_cap_face_id = build_cap_face(
        &mut topo,
        &mut geom,
        &cap_top,
        profile.normal.as_ref(),
        false,
        &mut he_map,
        quantize_pt,
    );
    all_faces.push(top_cap_face_id);

    pair_twin_half_edges(&mut topo, &he_map);

    let shell = topo.add_shell(all_faces, ShellType::Outer);
    let solid_id = topo.add_solid(shell);

    BRepSolid {
        topology: topo,
        geometry: geom,
        solid_id,
    }
}

/// Extrude a closed profile with interior holes along a direction.
///
/// The holes are given as closed loops of segments expressed in the *outer
/// profile's* 2D sketch coordinate system. Each hole loop becomes a ring of
/// interior lateral wall faces, and the two cap faces carry one inner loop
/// per hole, so the result is a single multiply-connected solid — no boolean
/// `Difference` pass is needed.
///
/// Winding is normalized internally: the outer loop is made counter-
/// clockwise and hole loops clockwise (viewed from the +normal direction),
/// so callers may pass loops in either orientation.
///
/// Arc segments (in the outer profile or holes) are polygonized with the
/// default [`ExtrudeOptions::arc_segments`] subdivision; the analytic
/// cylinder fast paths only apply to hole-free profiles.
///
/// # Preconditions
///
/// Hole loops must lie strictly inside the outer profile and must not touch
/// or overlap each other. This is not validated (it would cost more than the
/// extrusion itself); violating it yields self-intersecting geometry.
///
/// # Errors
///
/// Returns an error if the direction is zero, or if any hole loop is empty,
/// degenerate, or not closed.
pub fn extrude_with_holes(
    profile: &SketchProfile,
    holes: &[Vec<SketchSegment>],
    direction: Vec3,
) -> Result<BRepSolid, SketchError> {
    if holes.is_empty() {
        return extrude(profile, direction);
    }
    let dir_len = direction.norm();
    if dir_len < 1e-12 {
        return Err(SketchError::ZeroExtrusion);
    }

    let arc_segs = ExtrudeOptions::default().arc_segments as usize;

    // Validate each hole loop (closure, degeneracy) by round-tripping it
    // through the SketchProfile constructor on the outer profile's plane,
    // then polygonize arcs so the build below is line-only.
    let mut hole_profiles: Vec<SketchProfile> = Vec::with_capacity(holes.len());
    for hole in holes {
        let hp = SketchProfile::new(
            profile.origin,
            *profile.x_dir.as_ref(),
            *profile.y_dir.as_ref(),
            hole.clone(),
        )?;
        hole_profiles.push(if hp.is_line_only() {
            hp
        } else {
            hp.tessellate(arc_segs)
        });
    }
    let outer = if profile.is_line_only() {
        profile.clone()
    } else {
        profile.tessellate(arc_segs)
    };

    // Normalize winding: outer CCW, holes CW (viewed from +normal). The
    // lateral-face winding below then produces outward normals on the outer
    // walls and hole-facing normals on the interior walls.
    let outer_pts = oriented_loop_points(&outer, true);
    let hole_pts: Vec<Vec<Point2>> = hole_profiles
        .iter()
        .map(|hp| oriented_loop_points(hp, false))
        .collect();

    let mut topo = Topology::new();
    let mut geom = GeometryStore::new();

    let quantize_pt = |p: Point3| -> [i64; 3] {
        [
            (p.x * 1e9).round() as i64,
            (p.y * 1e9).round() as i64,
            (p.z * 1e9).round() as i64,
        ]
    };
    let mut vertex_cache: HashMap<[i64; 3], VertexId> = HashMap::new();
    let mut get_or_create = |topo: &mut Topology, pos: Point3| -> VertexId {
        let key = quantize_pt(pos);
        *vertex_cache
            .entry(key)
            .or_insert_with(|| topo.add_vertex(pos))
    };

    // Bottom/top vertex rings for the outer loop and each hole loop.
    let make_rings = |topo: &mut Topology,
                      cache: &mut dyn FnMut(&mut Topology, Point3) -> VertexId,
                      pts: &[Point2]|
     -> (Vec<VertexId>, Vec<VertexId>) {
        let mut bot = Vec::with_capacity(pts.len());
        let mut top = Vec::with_capacity(pts.len());
        for p2 in pts {
            let p3 = profile.to_3d(*p2);
            bot.push(cache(topo, p3));
            top.push(cache(topo, p3 + direction));
        }
        (bot, top)
    };

    let (outer_bot, outer_top) = make_rings(&mut topo, &mut get_or_create, &outer_pts);
    let hole_rings: Vec<(Vec<VertexId>, Vec<VertexId>)> = hole_pts
        .iter()
        .map(|pts| make_rings(&mut topo, &mut get_or_create, pts))
        .collect();

    let mut all_faces = Vec::new();
    let mut he_map: HashMap<([i64; 3], [i64; 3]), HalfEdgeId> = HashMap::new();

    // Lateral faces: one planar quad per loop edge, winding
    // bot_i → bot_next → top_next → top_i. For the CCW outer loop this
    // points normals out of the solid; for CW hole loops it points them
    // into the hole cavity (also out of the material).
    let build_lateral_ring =
        |topo: &mut Topology,
         geom: &mut GeometryStore,
         bot: &[VertexId],
         top: &[VertexId],
         faces: &mut Vec<vcad_kernel_topo::FaceId>,
         he_map: &mut HashMap<([i64; 3], [i64; 3]), HalfEdgeId>| {
            let n = bot.len();
            for i in 0..n {
                let j = (i + 1) % n;
                let p0 = topo.vertices[bot[i]].point;
                let p1 = topo.vertices[bot[j]].point;
                let p2 = topo.vertices[top[j]].point;
                let p3 = topo.vertices[top[i]].point;
                let (face_id, face_hes) = build_planar_lateral_face(
                    topo, geom, bot[i], bot[j], top[j], top[i], p0, p1, p2, p3,
                );
                faces.push(face_id);
                for he_id in face_hes {
                    let he = &topo.half_edges[he_id];
                    let origin = topo.vertices[he.origin].point;
                    let next = he.next.unwrap();
                    let dest = topo.vertices[topo.half_edges[next].origin].point;
                    he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
                }
            }
        };

    build_lateral_ring(
        &mut topo,
        &mut geom,
        &outer_bot,
        &outer_top,
        &mut all_faces,
        &mut he_map,
    );
    for (bot, top) in &hole_rings {
        build_lateral_ring(&mut topo, &mut geom, bot, top, &mut all_faces, &mut he_map);
    }

    // Caps. Bottom cap: outward normal is -profile.normal, so both the outer
    // loop and the hole loops are reversed relative to their stored order;
    // top cap keeps the stored order. Reversal keeps every cap half-edge
    // anti-parallel to its lateral twin so `pair_twin_half_edges` closes the
    // manifold.
    for (is_bottom, cap_normal) in [
        (true, -*profile.normal.as_ref()),
        (false, *profile.normal.as_ref()),
    ] {
        let ring = |bot: &Vec<VertexId>, top: &Vec<VertexId>| -> Vec<VertexId> {
            if is_bottom {
                bot.clone()
            } else {
                top.clone()
            }
        };
        let outer_ring = ring(&outer_bot, &outer_top);
        let origin_pos = topo.vertices[outer_ring[0]].point;
        let surf_idx = geom.add_surface(Box::new(Plane::from_normal(origin_pos, cap_normal)));

        let outer_loop_id =
            add_cap_loop(&mut topo, &outer_ring, is_bottom, &mut he_map, quantize_pt);
        let face_id = topo.add_face(outer_loop_id, surf_idx, Orientation::Forward);
        for (bot, top) in &hole_rings {
            let hole_ring = ring(bot, top);
            let loop_id = add_cap_loop(&mut topo, &hole_ring, is_bottom, &mut he_map, quantize_pt);
            topo.add_inner_loop(face_id, loop_id);
        }
        all_faces.push(face_id);
    }

    pair_twin_half_edges(&mut topo, &he_map);

    let shell = topo.add_shell(all_faces, ShellType::Outer);
    let solid_id = topo.add_solid(shell);

    Ok(BRepSolid {
        topology: topo,
        geometry: geom,
        solid_id,
    })
}

/// Segment start points of a (line-only) profile, oriented CCW when `ccw`
/// is true and CW otherwise, as measured in the profile's 2D plane.
fn oriented_loop_points(profile: &SketchProfile, ccw: bool) -> Vec<Point2> {
    let mut pts = profile.vertices_2d();
    let is_ccw = profile.signed_area() > 0.0;
    if is_ccw != ccw {
        pts.reverse();
    }
    pts
}

/// Create a cap loop over `ring` (reversed when `reversed`), registering its
/// half-edges in `he_map` for twin pairing. Returns the loop id; the caller
/// attaches it to a face as the outer or an inner loop.
fn add_cap_loop<F>(
    topo: &mut Topology,
    ring: &[VertexId],
    reversed: bool,
    he_map: &mut HashMap<([i64; 3], [i64; 3]), HalfEdgeId>,
    quantize_pt: F,
) -> vcad_kernel_topo::LoopId
where
    F: Fn(Point3) -> [i64; 3],
{
    let ordered: Vec<VertexId> = if reversed {
        ring.iter().rev().copied().collect()
    } else {
        ring.to_vec()
    };
    let hes: Vec<HalfEdgeId> = ordered.iter().map(|&v| topo.add_half_edge(v)).collect();
    let loop_id = topo.add_loop(&hes);
    for &he_id in &hes {
        let he = &topo.half_edges[he_id];
        let origin = topo.vertices[he.origin].point;
        let next = he.next.unwrap();
        let dest = topo.vertices[topo.half_edges[next].origin].point;
        he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
    }
    loop_id
}

/// Sample an arc from `start` to `end` with center `center`. Returns
/// `arc_segs + 1` points in 2D sketch space including both endpoints.
fn sample_arc_2d(
    start: Point2,
    end: Point2,
    center: Point2,
    ccw: bool,
    arc_segs: usize,
) -> Vec<Point2> {
    let start_vec = start - center;
    let end_vec = end - center;
    let radius = start_vec.norm();
    let start_angle = start_vec.y.atan2(start_vec.x);
    let mut end_angle = end_vec.y.atan2(end_vec.x);
    if ccw {
        while end_angle <= start_angle {
            end_angle += 2.0 * PI;
        }
    } else {
        while end_angle >= start_angle {
            end_angle -= 2.0 * PI;
        }
    }
    let mut pts = Vec::with_capacity(arc_segs + 1);
    pts.push(start);
    for i in 1..arc_segs {
        let t = i as f64 / arc_segs as f64;
        let angle = start_angle + t * (end_angle - start_angle);
        pts.push(Point2::new(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        ));
    }
    pts.push(end);
    pts
}

/// Extrude a closed profile with twist and/or scale (taper).
///
/// # Arguments
///
/// * `profile` - The closed 2D profile to extrude
/// * `direction` - The extrusion direction vector (magnitude = distance)
/// * `options` - Extrusion options (twist_angle, scale_end, arc_segments)
///
/// # Returns
///
/// A B-rep solid with lateral faces that are bilinear surfaces when twisted.
///
/// # Fast Path
///
/// When twist_angle is 0 and scale_end is 1.0, delegates to the standard
/// `extrude()` function for optimal performance.
///
/// # Example
///
/// ```
/// use vcad_kernel_sketch::{SketchProfile, extrude_with_options, ExtrudeOptions};
/// use vcad_kernel_math::{Point3, Vec3};
/// use std::f64::consts::PI;
///
/// let profile = SketchProfile::rectangle(
///     Point3::origin(),
///     Vec3::x(),
///     Vec3::y(),
///     10.0, 5.0,
/// );
///
/// // Extrude with 90° twist
/// let options = ExtrudeOptions {
///     twist_angle: PI / 2.0,
///     scale_end: 1.0,
///     ..Default::default()
/// };
/// let solid = extrude_with_options(&profile, Vec3::new(0.0, 0.0, 20.0), options).unwrap();
/// ```
pub fn extrude_with_options(
    profile: &SketchProfile,
    direction: Vec3,
    options: ExtrudeOptions,
) -> Result<BRepSolid, SketchError> {
    // Fast path: no twist or scale, use standard extrude
    if options.twist_angle.abs() < 1e-12 && (options.scale_end - 1.0).abs() < 1e-12 {
        return extrude(profile, direction);
    }

    let dir_len = direction.norm();
    if dir_len < 1e-12 {
        return Err(SketchError::ZeroExtrusion);
    }

    if profile.segments.is_empty() {
        return Err(SketchError::EmptyProfile);
    }

    // Calculate number of segments based on twist angle
    // ~12 segments per 90 degrees of twist, minimum 8
    let n_path_segments = if options.twist_angle.abs() < 1e-6 {
        8
    } else {
        ((options.twist_angle.abs() / (PI / 2.0)) * 12.0)
            .ceil()
            .max(8.0) as usize
    };
    let n_path_samples = n_path_segments + 1;

    // Tessellate arcs in the profile for smooth curves
    let arc_segments = options.arc_segments.max(1) as usize;
    let tessellated_profile = profile.tessellate(arc_segments);
    let n_profile_verts = tessellated_profile.segments.len();
    let profile_verts_2d = tessellated_profile.vertices_2d();

    // Build a simple linear frame system for the extrusion
    // Tangent is the direction, normal/binormal are profile X/Y axes
    let _tangent = Dir3::new_normalize(direction);

    // Use profile's X and Y directions as initial normal/binormal
    let normal = Dir3::new_normalize(*profile.x_dir.as_ref());
    let binormal = Dir3::new_normalize(*profile.y_dir.as_ref());

    let mut topo = Topology::new();
    let mut geom = GeometryStore::new();

    // Build vertex grid: [path_sample][profile_vertex]
    let mut vertex_grid: Vec<Vec<VertexId>> = Vec::with_capacity(n_path_samples);

    for path_idx in 0..n_path_samples {
        let t = path_idx as f64 / (n_path_samples - 1) as f64;

        // Position along extrusion
        let position = profile.origin + t * direction;

        // Twist angle at this position
        let twist = options.twist_angle * t;
        let (sin_a, cos_a) = twist.sin_cos();

        // Rotate normal and binormal around tangent
        let twisted_normal = cos_a * normal.as_ref() + sin_a * binormal.as_ref();
        let twisted_binormal = -sin_a * normal.as_ref() + cos_a * binormal.as_ref();

        // Scale factor at this position
        let scale = 1.0 + t * (options.scale_end - 1.0);

        let mut ring_verts = Vec::with_capacity(n_profile_verts);
        for p2d in &profile_verts_2d {
            let p3d = position + scale * (p2d.x * twisted_normal + p2d.y * twisted_binormal);
            let v_id = topo.add_vertex(p3d);
            ring_verts.push(v_id);
        }
        vertex_grid.push(ring_verts);
    }

    // Build faces
    let mut all_faces = Vec::new();
    let mut he_map: HashMap<([i64; 3], [i64; 3]), HalfEdgeId> = HashMap::new();

    let quantize_pt = |p: Point3| -> [i64; 3] {
        [
            (p.x * 1e9).round() as i64,
            (p.y * 1e9).round() as i64,
            (p.z * 1e9).round() as i64,
        ]
    };

    // Build lateral faces (one quad per profile edge × path segment)
    for path_idx in 0..n_path_segments {
        for profile_idx in 0..n_profile_verts {
            let next_profile_idx = (profile_idx + 1) % n_profile_verts;

            // Quad vertices (winding for outward normal):
            // v0 (this ring, this profile) -> v1 (this ring, next profile)
            // -> v2 (next ring, next profile) -> v3 (next ring, this profile)
            let v0 = vertex_grid[path_idx][profile_idx];
            let v1 = vertex_grid[path_idx][next_profile_idx];
            let v2 = vertex_grid[path_idx + 1][next_profile_idx];
            let v3 = vertex_grid[path_idx + 1][profile_idx];

            let p0 = topo.vertices[v0].point;
            let p1 = topo.vertices[v1].point;
            let p2 = topo.vertices[v2].point;
            let p3 = topo.vertices[v3].point;

            // Use BilinearSurface for twisted faces, Plane for planar ones
            let bilinear = BilinearSurface::new(p0, p1, p3, p2);
            let surf_idx = if bilinear.is_planar() {
                geom.add_surface(Box::new(Plane::new(p0, p1 - p0, p3 - p0)))
            } else {
                geom.add_surface(Box::new(bilinear))
            };

            // Create half-edges
            let he0 = topo.add_half_edge(v0);
            let he1 = topo.add_half_edge(v1);
            let he2 = topo.add_half_edge(v2);
            let he3 = topo.add_half_edge(v3);

            let loop_id = topo.add_loop(&[he0, he1, he2, he3]);
            let face_id = topo.add_face(loop_id, surf_idx, Orientation::Forward);
            all_faces.push(face_id);

            // Record half-edges for twin pairing
            for he_id in [he0, he1, he2, he3] {
                let he = &topo.half_edges[he_id];
                let origin = topo.vertices[he.origin].point;
                let next = he.next.unwrap();
                let dest = topo.vertices[topo.half_edges[next].origin].point;
                he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
            }
        }
    }

    // Build start cap (first ring, reversed winding for outward normal in -direction)
    let start_ring = &vertex_grid[0];
    let start_face_id = build_cap_face_twisted(
        &mut topo,
        &mut geom,
        start_ring,
        true,
        &mut he_map,
        quantize_pt,
    );
    all_faces.push(start_face_id);

    // Build end cap (last ring, forward winding for outward normal in +direction)
    let end_ring = &vertex_grid[n_path_samples - 1];
    let end_face_id = build_cap_face_twisted(
        &mut topo,
        &mut geom,
        end_ring,
        false,
        &mut he_map,
        quantize_pt,
    );
    all_faces.push(end_face_id);

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

fn build_cap_face_twisted<F>(
    topo: &mut Topology,
    geom: &mut GeometryStore,
    verts: &[VertexId],
    reversed: bool,
    he_map: &mut HashMap<([i64; 3], [i64; 3]), HalfEdgeId>,
    quantize_pt: F,
) -> vcad_kernel_topo::FaceId
where
    F: Fn(Point3) -> [i64; 3],
{
    let n = verts.len();
    let positions: Vec<Point3> = verts.iter().map(|&v| topo.vertices[v].point).collect();

    // Compute polygon normal using Newell's method
    let normal = compute_polygon_normal(&positions);

    let origin = positions[0];
    let surf_idx = if n >= 3 {
        let x_dir = positions[1] - origin;
        let y_dir = positions[n - 1] - origin;
        if x_dir.norm() > 1e-12 && y_dir.norm() > 1e-12 && x_dir.cross(y_dir).norm() > 1e-12 {
            geom.add_surface(Box::new(Plane::new(origin, x_dir, y_dir)))
        } else {
            geom.add_surface(Box::new(Plane::from_normal(origin, normal)))
        }
    } else {
        geom.add_surface(Box::new(Plane::from_normal(origin, normal)))
    };

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
        let next = he.next.unwrap();
        let dest = topo.vertices[topo.half_edges[next].origin].point;
        he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
    }

    face_id
}

fn compute_polygon_normal(verts: &[Point3]) -> Vec3 {
    if verts.len() < 3 {
        return Vec3::z();
    }

    // Newell's method for computing polygon normal
    let mut n = Vec3::zeros();
    for i in 0..verts.len() {
        let current = verts[i];
        let next = verts[(i + 1) % verts.len()];
        n.x += (current.y - next.y) * (current.z + next.z);
        n.y += (current.z - next.z) * (current.x + next.x);
        n.z += (current.x - next.x) * (current.y + next.y);
    }

    if n.norm() < 1e-12 {
        Vec3::z()
    } else {
        n.normalize()
    }
}

#[allow(clippy::too_many_arguments)]
fn build_planar_lateral_face(
    topo: &mut Topology,
    geom: &mut GeometryStore,
    v0: VertexId,
    v1: VertexId,
    v2: VertexId,
    v3: VertexId,
    p0: Point3,
    p1: Point3,
    p2: Point3,
    _p3: Point3,
) -> (vcad_kernel_topo::FaceId, Vec<HalfEdgeId>) {
    // Create plane from first three points
    // v0->v1 is bottom edge, v1->v2 is right edge going up
    let x_dir = p1 - p0; // along profile segment
    let y_dir = p2 - p1; // up along extrusion
    let surf_idx = geom.add_surface(Box::new(Plane::new(p0, x_dir, y_dir)));

    // Create half-edges: v0 -> v1 -> v2 -> v3 -> (back to v0)
    let he0 = topo.add_half_edge(v0);
    let he1 = topo.add_half_edge(v1);
    let he2 = topo.add_half_edge(v2);
    let he3 = topo.add_half_edge(v3);

    let loop_id = topo.add_loop(&[he0, he1, he2, he3]);
    let face_id = topo.add_face(loop_id, surf_idx, Orientation::Forward);

    (face_id, vec![he0, he1, he2, he3])
}

#[allow(clippy::too_many_arguments)]
fn build_cylindrical_lateral_face(
    topo: &mut Topology,
    geom: &mut GeometryStore,
    _profile: &SketchProfile,
    v0: VertexId,
    v1: VertexId,
    v2: VertexId,
    v3: VertexId,
    _center_2d: Point2,
    _ccw: bool,
    _direction: &Vec3,
) -> (vcad_kernel_topo::FaceId, Vec<HalfEdgeId>) {
    // For arc segments, we approximate with a planar face.
    // A true cylindrical surface would require the tessellator to handle
    // partial cylinder arcs, which is not currently supported.
    // Using a planar quad gives correct topology and approximately correct volume
    // for small arc segments.

    let p0 = topo.vertices[v0].point;
    let p1 = topo.vertices[v1].point;
    let p2 = topo.vertices[v2].point;

    // Create plane from the quad vertices
    let x_dir = p1 - p0;
    let y_dir = p2 - p1;
    let surf_idx = geom.add_surface(Box::new(Plane::new(p0, x_dir, y_dir)));

    // Same winding as other lateral faces: v0 -> v1 -> v2 -> v3
    let he0 = topo.add_half_edge(v0);
    let he1 = topo.add_half_edge(v1);
    let he2 = topo.add_half_edge(v2);
    let he3 = topo.add_half_edge(v3);

    let loop_id = topo.add_loop(&[he0, he1, he2, he3]);
    let face_id = topo.add_face(loop_id, surf_idx, Orientation::Forward);

    (face_id, vec![he0, he1, he2, he3])
}

fn build_cap_face<F>(
    topo: &mut Topology,
    geom: &mut GeometryStore,
    verts: &[VertexId],
    normal: &Vec3,
    reversed: bool,
    he_map: &mut HashMap<([i64; 3], [i64; 3]), HalfEdgeId>,
    quantize_pt: F,
) -> vcad_kernel_topo::FaceId
where
    F: Fn(Point3) -> [i64; 3],
{
    // Get positions
    let positions: Vec<Point3> = verts.iter().map(|&v| topo.vertices[v].point).collect();

    // Create plane surface — use `normal` directly so the plane's
    // `normal_dir` always points in the caller-specified outward
    // direction, regardless of the profile vertex winding. Downstream
    // consumers (fillet torus builder, tessellator) rely on
    // `plane.normal_dir` being the cap's true outward normal, not an
    // arbitrary direction that depends on whether positions[1] - [0]
    // crossed with positions[n-1] - [0] happens to point out or in.
    let origin = positions[0];
    let surf_idx = geom.add_surface(Box::new(Plane::from_normal(origin, *normal)));

    // Create half-edges in the correct order
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

    // Record half-edges for twin pairing
    for &he_id in &hes {
        let he = &topo.half_edges[he_id];
        let origin = topo.vertices[he.origin].point;
        let next = he.next.unwrap();
        let dest = topo.vertices[topo.half_edges[next].origin].point;
        he_map.insert((quantize_pt(origin), quantize_pt(dest)), he_id);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;
    use vcad_kernel_geom::SurfaceKind;

    #[test]
    fn test_extrude_rectangle() {
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 5.0);

        let solid = extrude(&profile, Vec3::new(0.0, 0.0, 20.0)).unwrap();

        // 6 faces: 4 lateral + 2 caps
        assert_eq!(solid.topology.faces.len(), 6);

        // 8 vertices
        assert_eq!(solid.topology.vertices.len(), 8);

        // 12 edges
        assert_eq!(solid.topology.edges.len(), 12);

        // All faces should be planar
        for surface in &solid.geometry.surfaces {
            assert_eq!(surface.surface_type(), SurfaceKind::Plane);
        }
    }

    #[test]
    fn test_extrude_rectangle_volume() {
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 5.0);

        let solid = extrude(&profile, Vec3::new(0.0, 0.0, 20.0)).unwrap();

        let mesh = vcad_kernel_tessellate::tessellate_brep(&solid, 32);
        let vol = compute_mesh_volume(&mesh);

        // Expected: 10 * 5 * 20 = 1000
        assert!(
            (vol - 1000.0).abs() < 1.0,
            "expected volume ~1000, got {vol}"
        );
    }

    #[test]
    fn test_extrude_all_halfedges_paired() {
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 5.0);

        let solid = extrude(&profile, Vec3::new(0.0, 0.0, 20.0)).unwrap();

        // All half-edges should have twins (closed manifold)
        let unpaired: Vec<_> = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .collect();

        assert!(
            unpaired.is_empty(),
            "found {} unpaired half-edges",
            unpaired.len()
        );
    }

    #[test]
    fn test_extrude_l_shape() {
        // L-shaped profile: 6 vertices
        let segments = vec![
            SketchSegment::Line {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(10.0, 0.0),
            },
            SketchSegment::Line {
                start: Point2::new(10.0, 0.0),
                end: Point2::new(10.0, 5.0),
            },
            SketchSegment::Line {
                start: Point2::new(10.0, 5.0),
                end: Point2::new(5.0, 5.0),
            },
            SketchSegment::Line {
                start: Point2::new(5.0, 5.0),
                end: Point2::new(5.0, 10.0),
            },
            SketchSegment::Line {
                start: Point2::new(5.0, 10.0),
                end: Point2::new(0.0, 10.0),
            },
            SketchSegment::Line {
                start: Point2::new(0.0, 10.0),
                end: Point2::new(0.0, 0.0),
            },
        ];

        let profile = SketchProfile::new(Point3::origin(), Vec3::x(), Vec3::y(), segments).unwrap();

        let solid = extrude(&profile, Vec3::new(0.0, 0.0, 15.0)).unwrap();

        // 8 faces: 6 lateral + 2 caps
        assert_eq!(solid.topology.faces.len(), 8);

        // 12 vertices (6 bottom + 6 top)
        assert_eq!(solid.topology.vertices.len(), 12);

        // All half-edges paired
        let unpaired = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .count();
        assert_eq!(unpaired, 0);
    }

    #[test]
    fn test_extrude_circle() {
        let profile = SketchProfile::circle(Point3::origin(), Vec3::z(), 5.0, 8);

        let solid = extrude(&profile, Vec3::new(0.0, 0.0, 10.0)).unwrap();

        // Analytic cylinder: 1 CylinderSurface lateral face + 2 planar disk caps.
        assert_eq!(solid.topology.faces.len(), 3);
        assert_eq!(
            solid
                .geometry
                .surfaces
                .iter()
                .filter(|s| s.surface_type() == SurfaceKind::Cylinder)
                .count(),
            1
        );

        // All half-edges should be paired
        let unpaired = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .count();
        assert_eq!(unpaired, 0, "expected no unpaired half-edges");

        // Volume should approximate π*r²*h = π*25*10 ≈ 785
        let mesh = vcad_kernel_tessellate::tessellate_brep(&solid, 64);
        let vol = compute_mesh_volume(&mesh);
        let expected = PI * 25.0 * 10.0;
        assert!(
            (vol - expected).abs() < expected * 0.01,
            "expected volume ~{expected:.1}, got {vol:.1}"
        );
    }

    #[test]
    fn test_extrude_two_arc_circle_is_not_a_rectangle() {
        // The AI chat path emits a full circle as exactly two half-arcs
        // (start=(r,0)→end=(-r,0), then start=(-r,0)→end=(r,0)). Without
        // arc tessellation, extrude() builds cap faces from only the 2
        // segment-start points and approximates each arc as a planar
        // quad, producing a degenerate "rectangle" instead of a cylinder.
        let r = 15.0;
        let h = 1.0;
        let segments = vec![
            SketchSegment::Arc {
                start: Point2::new(r, 0.0),
                end: Point2::new(-r, 0.0),
                center: Point2::origin(),
                ccw: false,
            },
            SketchSegment::Arc {
                start: Point2::new(-r, 0.0),
                end: Point2::new(r, 0.0),
                center: Point2::origin(),
                ccw: false,
            },
        ];
        let profile = SketchProfile::new(Point3::origin(), Vec3::x(), Vec3::y(), segments).unwrap();

        let solid = extrude(&profile, Vec3::new(0.0, 0.0, h)).unwrap();

        let mesh = vcad_kernel_tessellate::tessellate_brep(&solid, 64);
        let vol = compute_mesh_volume(&mesh);
        let expected = PI * r * r * h;
        assert!(
            (vol - expected).abs() < expected * 0.1,
            "two-arc circle should extrude to a cylinder (vol≈{expected:.1}), got {vol:.1} — likely built as a rectangle from diameter endpoints"
        );
    }

    #[test]
    fn test_extrude_zero_direction_error() {
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 5.0);

        let result = extrude(&profile, Vec3::zeros());
        assert!(matches!(result, Err(SketchError::ZeroExtrusion)));
    }

    #[test]
    fn test_extrude_angled_direction() {
        // Extrude at 45 degrees
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 10.0);

        let solid = extrude(&profile, Vec3::new(0.0, 10.0, 10.0)).unwrap();

        assert_eq!(solid.topology.faces.len(), 6);

        // For angled extrusion: V = A * (d · n) where d is direction, n is normal
        // d = (0, 10, 10), n = (0, 0, 1), so d·n = 10
        // V = 100 * 10 = 1000
        let mesh = vcad_kernel_tessellate::tessellate_brep(&solid, 32);
        let vol = compute_mesh_volume(&mesh);
        assert!(
            (vol - 1000.0).abs() < 5.0,
            "expected volume ~1000, got {vol}"
        );
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

    // =========================================================================
    // Tests for extrude_with_holes
    // =========================================================================

    /// Closed rectangle loop (CCW) as line segments.
    fn rect_loop(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<SketchSegment> {
        let p = [
            Point2::new(x0, y0),
            Point2::new(x1, y0),
            Point2::new(x1, y1),
            Point2::new(x0, y1),
        ];
        (0..4)
            .map(|i| SketchSegment::Line {
                start: p[i],
                end: p[(i + 1) % 4],
            })
            .collect()
    }

    /// Closed polygonal circle loop (CCW) as `n` line segments.
    fn circle_loop(cx: f64, cy: f64, r: f64, n: usize) -> Vec<SketchSegment> {
        let pt = |i: usize| {
            let a = 2.0 * PI * (i % n) as f64 / n as f64;
            Point2::new(cx + r * a.cos(), cy + r * a.sin())
        };
        (0..n)
            .map(|i| SketchSegment::Line {
                start: pt(i),
                end: pt(i + 1),
            })
            .collect()
    }

    fn mesh_volume_of(solid: &BRepSolid) -> f64 {
        let mesh = vcad_kernel_tessellate::tessellate_brep(solid, 32);
        compute_mesh_volume(&mesh)
    }

    #[test]
    fn test_extrude_with_holes_donut_volume_matches_boolean() {
        // Polygonal donut: 64-gon outer, 64-gon hole. The same loops drive
        // both the native holed extrude and the boolean Difference path, so
        // the volumes must agree to float noise.
        let h = 4.0;
        let outer = SketchProfile::new(
            Point3::origin(),
            Vec3::x(),
            Vec3::y(),
            circle_loop(0.0, 0.0, 10.0, 64),
        )
        .unwrap();
        let hole = circle_loop(0.0, 0.0, 4.0, 64);

        let holed = extrude_with_holes(&outer, std::slice::from_ref(&hole), Vec3::new(0.0, 0.0, h))
            .unwrap();

        let outer_solid = extrude(&outer, Vec3::new(0.0, 0.0, h)).unwrap();
        let hole_profile =
            SketchProfile::new(Point3::origin(), Vec3::x(), Vec3::y(), hole).unwrap();
        let hole_solid = extrude(&hole_profile, Vec3::new(0.0, 0.0, h)).unwrap();

        let diff = vcad_kernel_booleans::boolean_op(
            &outer_solid,
            &hole_solid,
            vcad_kernel_booleans::BooleanOp::Difference,
            32,
        )
        .into_brep()
        .unwrap();
        let boolean = mesh_volume_of(&diff);
        let v_holed = mesh_volume_of(&holed);

        // Independent analytic check: prismatic volume = (A_outer − A_hole)·h.
        let a_outer = SketchProfile::new(
            Point3::origin(),
            Vec3::x(),
            Vec3::y(),
            circle_loop(0.0, 0.0, 10.0, 64),
        )
        .unwrap()
        .signed_area();
        let a_hole = SketchProfile::new(
            Point3::origin(),
            Vec3::x(),
            Vec3::y(),
            circle_loop(0.0, 0.0, 4.0, 64),
        )
        .unwrap()
        .signed_area();
        let expected = (a_outer - a_hole) * h;

        // TriangleMesh stores f32 vertices, so mesh-derived volumes carry
        // ~1e-7 relative noise; 1e-6 relative is the tightest meaningful
        // tolerance here (observed error ≈ 7e-9 relative).
        assert!(
            (v_holed - expected).abs() < 1e-6 * expected.max(1.0),
            "holed extrude volume {v_holed} != analytic {expected}"
        );
        assert!(
            (v_holed - boolean).abs() < 1e-6 * expected.max(1.0),
            "holed extrude volume {v_holed} != boolean path {boolean}"
        );
    }

    #[test]
    fn test_extrude_with_holes_two_hole_plate() {
        let outer = SketchProfile::new(
            Point3::origin(),
            Vec3::x(),
            Vec3::y(),
            rect_loop(0.0, 0.0, 30.0, 10.0),
        )
        .unwrap();
        let h1 = rect_loop(5.0, 3.0, 10.0, 7.0);
        let h2 = rect_loop(20.0, 2.0, 26.0, 8.0);
        let h = 2.5;

        let solid = extrude_with_holes(&outer, &[h1, h2], Vec3::new(0.0, 0.0, h)).unwrap();

        // Volume = (30·10 − 5·4 − 6·6) · 2.5
        let expected = (300.0 - 20.0 - 36.0) * h;
        let vol = mesh_volume_of(&solid);
        assert!(
            (vol - expected).abs() < 1e-6 * expected,
            "expected {expected}, got {vol}"
        );

        // Watertight: every half-edge twinned.
        let unpaired = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .count();
        assert_eq!(unpaired, 0, "found {unpaired} unpaired half-edges");

        // 4 outer + 2×4 hole lateral faces + 2 caps.
        assert_eq!(solid.topology.faces.len(), 14);
        // Caps carry 2 inner loops each.
        let caps_with_holes = solid
            .topology
            .faces
            .values()
            .filter(|f| f.inner_loops.len() == 2)
            .count();
        assert_eq!(caps_with_holes, 2);
    }

    #[test]
    fn test_extrude_with_holes_winding_invariance() {
        // Hole loops may be passed CW or CCW; result must be identical.
        let outer = SketchProfile::new(
            Point3::origin(),
            Vec3::x(),
            Vec3::y(),
            rect_loop(0.0, 0.0, 10.0, 10.0),
        )
        .unwrap();
        let hole_ccw = rect_loop(4.0, 4.0, 6.0, 6.0);
        let hole_cw: Vec<SketchSegment> = hole_ccw
            .iter()
            .rev()
            .map(|s| match s {
                SketchSegment::Line { start, end } => SketchSegment::Line {
                    start: *end,
                    end: *start,
                },
                _ => unreachable!(),
            })
            .collect();

        let v1 = mesh_volume_of(
            &extrude_with_holes(&outer, &[hole_ccw], Vec3::new(0.0, 0.0, 3.0)).unwrap(),
        );
        let v2 = mesh_volume_of(
            &extrude_with_holes(&outer, &[hole_cw], Vec3::new(0.0, 0.0, 3.0)).unwrap(),
        );
        let expected = (100.0 - 4.0) * 3.0;
        assert!((v1 - expected).abs() < 1e-6 * expected, "ccw hole: {v1}");
        assert!((v2 - expected).abs() < 1e-6 * expected, "cw hole: {v2}");
    }

    #[test]
    fn test_extrude_with_holes_empty_holes_is_plain_extrude() {
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 5.0);
        let solid = extrude_with_holes(&profile, &[], Vec3::new(0.0, 0.0, 20.0)).unwrap();
        assert_eq!(solid.topology.faces.len(), 6);
    }

    #[test]
    fn test_extrude_with_holes_open_hole_error() {
        let outer = SketchProfile::new(
            Point3::origin(),
            Vec3::x(),
            Vec3::y(),
            rect_loop(0.0, 0.0, 10.0, 10.0),
        )
        .unwrap();
        let open_hole = vec![SketchSegment::Line {
            start: Point2::new(2.0, 2.0),
            end: Point2::new(4.0, 2.0),
        }];
        let result = extrude_with_holes(&outer, &[open_hole], Vec3::new(0.0, 0.0, 3.0));
        assert!(matches!(result, Err(SketchError::NotClosed(_))));
    }

    // =========================================================================
    // Tests for extrude_with_options
    // =========================================================================

    #[test]
    fn test_extrude_with_twist_90_deg() {
        use super::*;
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 5.0);

        let options = ExtrudeOptions {
            twist_angle: PI / 2.0, // 90 degrees
            scale_end: 1.0,
            ..Default::default()
        };

        let solid = extrude_with_options(&profile, Vec3::new(0.0, 0.0, 20.0), options).unwrap();

        // Should have many faces due to twist segments
        assert!(solid.topology.faces.len() > 6);

        // All half-edges should be paired
        let unpaired = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .count();
        assert_eq!(unpaired, 0, "expected no unpaired half-edges");

        // Volume should be approximately the same as non-twisted (10 * 5 * 20 = 1000)
        // Allow 10% tolerance due to bilinear surface tessellation effects
        let mesh = vcad_kernel_tessellate::tessellate_brep(&solid, 32);
        let vol = compute_mesh_volume(&mesh);
        assert!(
            (vol - 1000.0).abs() < 100.0,
            "expected volume ~1000, got {vol}"
        );
    }

    #[test]
    fn test_extrude_with_scale_half() {
        use super::*;
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 10.0);

        let options = ExtrudeOptions {
            twist_angle: 0.0,
            scale_end: 0.5,
            ..Default::default()
        };

        let solid = extrude_with_options(&profile, Vec3::new(0.0, 0.0, 20.0), options).unwrap();

        // Should have faces
        assert!(solid.topology.faces.len() >= 6);

        // All half-edges should be paired
        let unpaired = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .count();
        assert_eq!(unpaired, 0, "expected no unpaired half-edges");

        // Volume of a truncated pyramid: V = h/3 * (A1 + A2 + sqrt(A1*A2))
        // A1 = 100, A2 = 25 (scale 0.5 squared), h = 20
        // V = 20/3 * (100 + 25 + 50) = 20/3 * 175 ≈ 1166.67
        let mesh = vcad_kernel_tessellate::tessellate_brep(&solid, 32);
        let vol = compute_mesh_volume(&mesh);
        let expected = 20.0 / 3.0 * (100.0 + 25.0 + 50.0);
        assert!(
            (vol - expected).abs() < expected * 0.15,
            "expected volume ~{expected:.1}, got {vol:.1}"
        );
    }

    #[test]
    fn test_extrude_with_twist_and_scale() {
        use super::*;
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 10.0);

        let options = ExtrudeOptions {
            twist_angle: PI / 4.0, // 45 degrees
            scale_end: 0.5,
            ..Default::default()
        };

        let solid = extrude_with_options(&profile, Vec3::new(0.0, 0.0, 20.0), options).unwrap();

        // Should have faces
        assert!(solid.topology.faces.len() > 6);

        // All half-edges should be paired
        let unpaired = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .count();
        assert_eq!(unpaired, 0, "expected no unpaired half-edges");
    }

    #[test]
    fn test_extrude_fast_path_no_twist_no_scale() {
        use super::*;
        let profile = SketchProfile::rectangle(Point3::origin(), Vec3::x(), Vec3::y(), 10.0, 5.0);

        let options = ExtrudeOptions::default(); // twist=0, scale=1.0

        let solid = extrude_with_options(&profile, Vec3::new(0.0, 0.0, 20.0), options).unwrap();

        // Should use fast path, same as regular extrude
        assert_eq!(solid.topology.faces.len(), 6);
        assert_eq!(solid.topology.vertices.len(), 8);
    }

    #[test]
    fn test_extrude_with_options_circle_profile() {
        use super::*;
        let profile = SketchProfile::circle(Point3::origin(), Vec3::z(), 5.0, 8);

        let options = ExtrudeOptions {
            twist_angle: PI, // 180 degrees
            scale_end: 0.8,
            arc_segments: 4,
        };

        let solid = extrude_with_options(&profile, Vec3::new(0.0, 0.0, 20.0), options).unwrap();

        // Should have many faces
        assert!(solid.topology.faces.len() > 10);

        // All half-edges should be paired
        let unpaired = solid
            .topology
            .half_edges
            .values()
            .filter(|he| he.twin.is_none())
            .count();
        assert_eq!(unpaired, 0, "expected no unpaired half-edges");
    }
}
