//! Curved fillet support — torus blends for plane-cylinder and coaxial cylinder cases.

use std::collections::HashMap;
use vcad_kernel_geom::{
    CylinderSurface, GeometryStore, Plane, SphereSurface, Surface, SurfaceKind, TorusSurface,
};
use vcad_kernel_math::{Dir3, Point3, Vec3};
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{EdgeId, FaceId, HalfEdgeId, Orientation, ShellType, Topology, VertexId};

use crate::fillet_planar::build_plane_plane_blend;
use crate::rolling_ball::rolling_ball_blend;
use crate::topology::{
    compute_centroid, compute_face_normal, extract_edges, extract_faces, pair_twin_half_edges,
    quantize, CurvedFaceInfo, EdgeInfo, FaceInfo,
};
use crate::trim::{build_vertex_faces, compute_trim_vertices, TrimKey};
use crate::{classify_fillet_case, FilletCase, FilletResult};

/// Fillet specific edges of a B-rep solid, supporting curved faces.
///
/// This is the extended fillet API that handles plane-cylinder, coaxial cylinder,
/// and general curved face pairs in addition to the basic plane-plane case.
///
/// # Arguments
///
/// * `brep` - The input solid
/// * `edge_ids` - Edges to fillet
/// * `radius` - Fillet radius
///
/// # Returns
///
/// A new `BRepSolid` and a vector of per-edge results indicating success or failure.
pub fn fillet_edges_detailed(
    brep: &BRepSolid,
    edge_ids: &[EdgeId],
    radius: f64,
) -> (BRepSolid, Vec<FilletResult>) {
    let faces = extract_faces(brep);
    let edges = extract_edges(brep);
    let topo = &brep.topology;
    let geom = &brep.geometry;

    let target_edges: Vec<&EdgeInfo> = edges
        .iter()
        .filter(|e| edge_ids.contains(&e.edge_id))
        .collect();

    if target_edges.is_empty() {
        return (brep.clone(), Vec::new());
    }

    // Build curved face info for classification
    let _curved_faces: HashMap<FaceId, CurvedFaceInfo> = topo
        .faces
        .iter()
        .map(|(face_id, face)| {
            let surface = &geom.surfaces[face.surface_index];
            let vertex_ids = topo.loop_vertices(face.outer_loop);
            let positions: Vec<Point3> =
                vertex_ids.iter().map(|&v| topo.vertices[v].point).collect();
            let planar_normal = if surface.surface_type() == SurfaceKind::Plane {
                Some(compute_face_normal(&positions))
            } else {
                None
            };
            (
                face_id,
                CurvedFaceInfo {
                    face_id,
                    surface_index: face.surface_index,
                    surface_kind: surface.surface_type(),
                    vertex_ids,
                    positions,
                    planar_normal,
                },
            )
        })
        .collect();

    let mut results = Vec::new();
    let trims = compute_trim_vertices(&faces, radius);
    let face_map: HashMap<FaceId, &FaceInfo> = faces.iter().map(|f| (f.face_id, f)).collect();

    let mut vertex_edges: HashMap<VertexId, Vec<&EdgeInfo>> = HashMap::new();
    for edge in &edges {
        vertex_edges.entry(edge.v_start).or_default().push(edge);
        vertex_edges.entry(edge.v_end).or_default().push(edge);
    }

    let mut new_topo = Topology::new();
    let mut new_geom = GeometryStore::new();
    let mut vertex_cache: HashMap<[i64; 3], VertexId> = HashMap::new();

    let get_or_create_vertex =
        |cache: &mut HashMap<[i64; 3], VertexId>, topo: &mut Topology, pos: Point3| -> VertexId {
            let key = quantize(pos);
            *cache.entry(key).or_insert_with(|| topo.add_vertex(pos))
        };

    let mut all_faces = Vec::new();

    // 1. Build modified original faces
    for face in &faces {
        let new_positions: Vec<Point3> = face
            .vertex_ids
            .iter()
            .filter_map(|&v_id| trims.get(&(v_id, face.face_id)).copied())
            .collect();

        if new_positions.len() < 3 {
            continue;
        }

        let verts: Vec<VertexId> = new_positions
            .iter()
            .map(|p| get_or_create_vertex(&mut vertex_cache, &mut new_topo, *p))
            .collect();

        let face_data = &topo.faces[face.face_id];
        let surface = &geom.surfaces[face_data.surface_index];
        let surf_idx = new_geom.add_surface(surface.clone_box());

        let hes: Vec<HalfEdgeId> = verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
        let loop_id = new_topo.add_loop(&hes);
        let face_id = new_topo.add_face(loop_id, surf_idx, face_data.orientation);
        all_faces.push(face_id);
    }

    // 2. Build blend faces for each target edge
    for edge_info in &target_edges {
        let surface_a = &geom.surfaces[topo.faces[edge_info.face_a].surface_index];
        let surface_b = &geom.surfaces[topo.faces[edge_info.face_b].surface_index];
        let case = classify_fillet_case(surface_a.as_ref(), surface_b.as_ref());

        match case {
            FilletCase::PlanePlane => {
                let fa = face_map.get(&edge_info.face_a);
                let fb = face_map.get(&edge_info.face_b);
                if let (Some(fa), Some(fb)) = (fa, fb) {
                    if build_plane_plane_blend(
                        edge_info,
                        fa,
                        fb,
                        &trims,
                        &faces,
                        radius,
                        brep,
                        &mut vertex_cache,
                        &mut new_topo,
                        &mut new_geom,
                        &mut all_faces,
                    ) {
                        results.push(FilletResult::Success);
                    } else {
                        results.push(FilletResult::DegenerateGeometry {
                            edge_id: edge_info.edge_id,
                        });
                    }
                }
            }
            FilletCase::PlaneCylinder => {
                let (plane_surf, cyl_surf, plane_face_id, _cyl_face_id) =
                    if surface_a.surface_type() == SurfaceKind::Plane {
                        (surface_a, surface_b, edge_info.face_a, edge_info.face_b)
                    } else {
                        (surface_b, surface_a, edge_info.face_b, edge_info.face_a)
                    };

                if let Some(torus) = build_plane_cylinder_torus(
                    plane_surf.as_ref(),
                    cyl_surf.as_ref(),
                    topo.faces[plane_face_id].orientation,
                    radius,
                ) {
                    if let Some(()) = build_blend_quad(
                        edge_info,
                        &trims,
                        &faces,
                        torus,
                        &mut vertex_cache,
                        &mut new_topo,
                        &mut new_geom,
                        &mut all_faces,
                    ) {
                        results.push(FilletResult::Success);
                    } else {
                        results.push(FilletResult::DegenerateGeometry {
                            edge_id: edge_info.edge_id,
                        });
                    }
                } else {
                    results.push(FilletResult::Unsupported {
                        edge_id: edge_info.edge_id,
                        reason: "could not construct torus blend".into(),
                    });
                }
            }
            FilletCase::CylinderCylinderCoaxial => {
                if let Some(torus) =
                    build_coaxial_cylinder_torus(surface_a.as_ref(), surface_b.as_ref(), radius)
                {
                    if let Some(()) = build_blend_quad(
                        edge_info,
                        &trims,
                        &faces,
                        torus,
                        &mut vertex_cache,
                        &mut new_topo,
                        &mut new_geom,
                        &mut all_faces,
                    ) {
                        results.push(FilletResult::Success);
                    } else {
                        results.push(FilletResult::DegenerateGeometry {
                            edge_id: edge_info.edge_id,
                        });
                    }
                } else {
                    results.push(FilletResult::Unsupported {
                        edge_id: edge_info.edge_id,
                        reason: "could not construct coaxial torus blend".into(),
                    });
                }
            }
            FilletCase::CylinderCylinderSkew | FilletCase::GeneralCurved => {
                let v_start_pos = topo.vertices[edge_info.v_start].point;
                let v_end_pos = topo.vertices[edge_info.v_end].point;

                match rolling_ball_blend(
                    surface_a.as_ref(),
                    surface_b.as_ref(),
                    v_start_pos,
                    v_end_pos,
                    radius,
                    8,
                    5,
                ) {
                    Some(bspline) => {
                        if let Some(()) = build_blend_quad_surface(
                            edge_info,
                            &trims,
                            &faces,
                            Box::new(bspline),
                            &mut vertex_cache,
                            &mut new_topo,
                            &mut new_geom,
                            &mut all_faces,
                        ) {
                            results.push(FilletResult::Success);
                        } else {
                            results.push(FilletResult::DegenerateGeometry {
                                edge_id: edge_info.edge_id,
                            });
                        }
                    }
                    None => {
                        results.push(FilletResult::Unsupported {
                            edge_id: edge_info.edge_id,
                            reason: format!("rolling ball blend failed for {:?} edge", case),
                        });
                    }
                }
            }
            FilletCase::Unsupported => {
                results.push(FilletResult::Unsupported {
                    edge_id: edge_info.edge_id,
                    reason: format!(
                        "unsupported surface combination: {:?} / {:?}",
                        surface_a.surface_type(),
                        surface_b.surface_type()
                    ),
                });
            }
        }
    }

    // 3. Build vertex faces for target edges
    let target_vertex_edges: HashMap<VertexId, Vec<&EdgeInfo>> = {
        let mut map: HashMap<VertexId, Vec<&EdgeInfo>> = HashMap::new();
        for &edge in &target_edges {
            map.entry(edge.v_start).or_default().push(edge);
            map.entry(edge.v_end).or_default().push(edge);
        }
        map
    };

    build_vertex_faces(
        &faces,
        &target_vertex_edges,
        &trims,
        brep,
        &mut vertex_cache,
        &mut new_topo,
        &mut new_geom,
        &mut all_faces,
    );

    // 3b. Spherical vertex blends at convex 3-edge junctions.
    // At every vertex where two filleted plane-cylinder edges meet and the
    // non-filleted third edge is a cylinder-cylinder seam (arc-extrude
    // junction), Fusion / SolidWorks emit a spherical patch tangent to all
    // three surfaces — the envelope of the rolling ball when it pivots at
    // the junction. Without it the two adjacent torus blends leave a
    // crescent gap where their interiors diverge. Build that patch as a
    // standalone triangular face on a new `SphereSurface` so the tessellator
    // renders the crescent.
    build_spherical_vertex_blends(
        brep,
        &edges,
        &target_edges,
        &faces,
        &trims,
        radius,
        &mut vertex_cache,
        &mut new_topo,
        &mut new_geom,
        &mut all_faces,
    );

    // 4. Pair twin half-edges and build shell
    pair_twin_half_edges(&mut new_topo);
    let shell = new_topo.add_shell(all_faces, ShellType::Outer);
    let solid_id = new_topo.add_solid(shell);

    (
        BRepSolid {
            topology: new_topo,
            geometry: new_geom,
            solid_id,
        },
        results,
    )
}

/// Build spherical vertex-blend patches at every convex 3-edge junction.
///
/// A convex junction here is a vertex where two filleted plane-cylinder
/// edges meet AND the non-filleted edge between the two cylinders is a
/// seam of the arc-extrude profile (cylinder-cylinder with parallel
/// axes, omitted from the fillet target list). At such a junction a
/// ball of radius `r` is tangent to the cap plane and both cylinders
/// simultaneously; its center plus three tangent points define the
/// spherical patch that fills the crescent between the adjacent torus
/// blends. We add each patch as a standalone triangular face on a new
/// `SphereSurface` — topologically disconnected from the surrounding
/// tori and cap, but positioned so the tessellated mesh covers the
/// visible gap. (Full setback trimming so the faces share explicit
/// edges with adjacent tori is a bigger surgery and can land on top of
/// this without reworking the underlying blend plumbing.)
#[allow(clippy::too_many_arguments)]
fn build_spherical_vertex_blends(
    brep: &BRepSolid,
    edges: &[EdgeInfo],
    target_edges: &[&EdgeInfo],
    faces: &[FaceInfo],
    _trims: &HashMap<TrimKey, Point3>,
    radius: f64,
    vertex_cache: &mut HashMap<[i64; 3], VertexId>,
    new_topo: &mut Topology,
    new_geom: &mut GeometryStore,
    all_faces: &mut Vec<FaceId>,
) {
    let topo = &brep.topology;
    let geom = &brep.geometry;

    // Index the target edges by each endpoint vertex.
    let mut target_by_vertex: HashMap<VertexId, Vec<&EdgeInfo>> = HashMap::new();
    for edge in target_edges {
        target_by_vertex.entry(edge.v_start).or_default().push(edge);
        target_by_vertex.entry(edge.v_end).or_default().push(edge);
    }

    // Also index ALL edges (including non-target seams) by endpoint.
    let mut all_by_vertex: HashMap<VertexId, Vec<&EdgeInfo>> = HashMap::new();
    for edge in edges {
        all_by_vertex.entry(edge.v_start).or_default().push(edge);
        all_by_vertex.entry(edge.v_end).or_default().push(edge);
    }

    for (v_id, tgt_edges) in &target_by_vertex {
        // Need exactly two filleted edges incident to this vertex.
        if tgt_edges.len() != 2 {
            continue;
        }

        // Classify each filleted edge as plane-cylinder and extract the
        // plane's outward normal + the cylinder.
        let mut plane_normal: Option<Vec3> = None;
        let mut cyls: Vec<CylinderSurface> = Vec::new();
        let mut cyl_faces: Vec<FaceId> = Vec::new();
        let mut cap_face_id: Option<FaceId> = None;
        let mut ok = true;
        for e in tgt_edges {
            let sa = &geom.surfaces[topo.faces[e.face_a].surface_index];
            let sb = &geom.surfaces[topo.faces[e.face_b].surface_index];
            let (plane_face, plane_surf, cyl_face, cyl_surf) = match (
                sa.surface_type(),
                sb.surface_type(),
            ) {
                (SurfaceKind::Plane, SurfaceKind::Cylinder) => (e.face_a, sa, e.face_b, sb),
                (SurfaceKind::Cylinder, SurfaceKind::Plane) => (e.face_b, sb, e.face_a, sa),
                _ => {
                    ok = false;
                    break;
                }
            };
            let plane = match plane_surf.as_any().downcast_ref::<Plane>() {
                Some(p) => p,
                None => {
                    ok = false;
                    break;
                }
            };
            let cyl = match cyl_surf.as_any().downcast_ref::<CylinderSurface>() {
                Some(c) => c.clone(),
                None => {
                    ok = false;
                    break;
                }
            };
            let outward = match topo.faces[plane_face].orientation {
                Orientation::Forward => *plane.normal_dir.as_ref(),
                Orientation::Reversed => -*plane.normal_dir.as_ref(),
            };
            match plane_normal {
                None => {
                    plane_normal = Some(outward);
                    cap_face_id = Some(plane_face);
                }
                Some(n) => {
                    // Both target edges must share the same cap plane.
                    if n.dot(outward) < 1.0 - 1e-6 {
                        ok = false;
                        break;
                    }
                }
            }
            cyls.push(cyl);
            cyl_faces.push(cyl_face);
        }
        if !ok {
            continue;
        }
        let plane_normal = match plane_normal {
            Some(n) => n,
            None => continue,
        };
        if cyls.len() != 2 {
            continue;
        }

        // Confirm the third incident edge is a cylinder-cylinder seam
        // (non-filleted) between the two cylinders we found. Otherwise
        // this isn't an arc-extrude convex junction we can blend.
        let incident = match all_by_vertex.get(v_id) {
            Some(v) => v,
            None => continue,
        };
        if incident.len() < 3 {
            continue;
        }
        let is_target = |e: &&EdgeInfo| {
            tgt_edges.iter().any(|t| t.edge_id == e.edge_id)
        };
        let seam_edges: Vec<&&EdgeInfo> =
            incident.iter().filter(|e| !is_target(e)).collect();
        let seam_connects_cyls = seam_edges.iter().any(|e| {
            let sa = &geom.surfaces[topo.faces[e.face_a].surface_index];
            let sb = &geom.surfaces[topo.faces[e.face_b].surface_index];
            sa.surface_type() == SurfaceKind::Cylinder
                && sb.surface_type() == SurfaceKind::Cylinder
        });
        if !seam_connects_cyls {
            continue;
        }

        // Solve for the rolling ball center. The ball must sit at
        // distance r from the cap plane on the interior side, and at
        // distance `R_i − r` from each cylinder's axis (convex arc →
        // solid lies inside the cylinder's radius). Interior direction
        // = -outward_plane_normal.
        let v_pos = topo.vertices[*v_id].point;
        let cap_center_t = v_pos + (-plane_normal) * radius;

        // Project the two cylinder axes onto the cap plane. For a Z-up
        // extrude all of this is trivially in the XY plane, but we keep
        // the full projection so arbitrary cap orientations also work.
        let offset_centers: Vec<Point3> = cyls
            .iter()
            .map(|c| {
                let d = cap_center_t - c.center;
                let along = d.dot(c.axis.as_ref());
                c.center + *c.axis.as_ref() * along
            })
            .collect();
        let offset_radii: Vec<f64> = cyls.iter().map(|c| c.radius - radius).collect();
        if offset_radii.iter().any(|r| *r <= 0.0) {
            continue;
        }

        let ball_center = match solve_two_circles_in_plane(
            &cap_center_t,
            &plane_normal,
            &offset_centers,
            &offset_radii,
            &v_pos,
        ) {
            Some(p) => p,
            None => continue,
        };

        // Tangent points: sphere meets cap at ball_center + r *
        // -outward_normal (closest point on sphere to cap plane;
        // radially OUTWARD from sphere toward plane). Sphere meets cyl_i
        // at the point on cyl_i's surface closest to ball_center.
        let tan_cap = ball_center + plane_normal * radius;
        let mut tan_cyls = Vec::with_capacity(2);
        for (c, r) in cyls.iter().zip(offset_radii.iter()) {
            let d = ball_center - c.center;
            let along = d.dot(c.axis.as_ref());
            let radial = d - *c.axis.as_ref() * along;
            let radial_len = radial.norm();
            if radial_len < 1e-9 {
                tan_cyls.clear();
                break;
            }
            let radial_unit = radial / radial_len;
            // Ball is INSIDE cylinder at distance r-radius from axis
            // on surface at the full cyl radius (R = r + radius).
            tan_cyls.push(c.center + *c.axis.as_ref() * along + radial_unit * c.radius);
            let _ = r;
        }
        if tan_cyls.len() != 2 {
            continue;
        }

        // Build the patch: a triangular face on a new SphereSurface with
        // vertices at the three tangent points.
        let get_or_create = |cache: &mut HashMap<[i64; 3], VertexId>,
                             topo: &mut Topology,
                             p: Point3|
         -> VertexId {
            let key = quantize(p);
            *cache.entry(key).or_insert_with(|| topo.add_vertex(p))
        };
        let v_cap = get_or_create(vertex_cache, new_topo, tan_cap);
        let v_c1 = get_or_create(vertex_cache, new_topo, tan_cyls[0]);
        let v_c2 = get_or_create(vertex_cache, new_topo, tan_cyls[1]);
        if v_cap == v_c1 || v_c1 == v_c2 || v_cap == v_c2 {
            continue;
        }

        let sphere = SphereSurface {
            center: ball_center,
            radius,
            ref_dir: Dir3::new_normalize(tan_cap - ball_center),
            axis: Dir3::new_normalize(-plane_normal),
        };

        // Wind the triangle so its outward normal faces AWAY from the
        // solid interior (i.e. roughly along the outward direction at
        // the vertex).
        let _ = cap_face_id;
        let solid_exterior_hint = v_pos - compute_centroid(faces);
        let e1 = tan_cyls[0] - tan_cap;
        let e2 = tan_cyls[1] - tan_cap;
        let n = e1.cross(e2);
        let verts: [VertexId; 3] = if n.dot(solid_exterior_hint) > 0.0 {
            [v_cap, v_c1, v_c2]
        } else {
            [v_cap, v_c2, v_c1]
        };

        let hes: Vec<HalfEdgeId> = verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
        let loop_id = new_topo.add_loop(&hes);
        let surf_idx = new_geom.add_surface(Box::new(sphere));
        let face_id = new_topo.add_face(loop_id, surf_idx, Orientation::Forward);
        all_faces.push(face_id);
    }
}

/// Solve for the point in a plane that lies at specified distances from
/// two projected centers — used for placing the rolling ball at a
/// convex junction. Picks the solution closer to `near_hint`. Returns
/// None if the circles don't intersect.
fn solve_two_circles_in_plane(
    plane_origin: &Point3,
    plane_normal: &Vec3,
    centers: &[Point3],
    radii: &[f64],
    near_hint: &Point3,
) -> Option<Point3> {
    if centers.len() != 2 || radii.len() != 2 {
        return None;
    }
    // Build an orthonormal (u, v) basis for the plane.
    let n = plane_normal.normalize();
    let base = if n.x.abs() < 0.9 {
        Vec3::new(1.0, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let u_axis = (base - n * n.dot(base)).normalize();
    let v_axis = n.cross(u_axis);

    let project = |p: Point3| -> (f64, f64) {
        let d = p - *plane_origin;
        (d.dot(u_axis), d.dot(v_axis))
    };
    let (cx1, cy1) = project(centers[0]);
    let (cx2, cy2) = project(centers[1]);
    let r1 = radii[0];
    let r2 = radii[1];

    let dx = cx2 - cx1;
    let dy = cy2 - cy1;
    let d = (dx * dx + dy * dy).sqrt();
    if d < 1e-12 || d > r1 + r2 || d < (r1 - r2).abs() {
        return None;
    }
    let a = (r1 * r1 - r2 * r2 + d * d) / (2.0 * d);
    let h2 = r1 * r1 - a * a;
    if h2 < -1e-9 {
        return None;
    }
    let h = h2.max(0.0).sqrt();
    let mid_x = cx1 + a * dx / d;
    let mid_y = cy1 + a * dy / d;
    let rx = -dy / d;
    let ry = dx / d;

    let candidates = [
        (mid_x + h * rx, mid_y + h * ry),
        (mid_x - h * rx, mid_y - h * ry),
    ];
    let (hx, hy) = project(*near_hint);
    let best = candidates
        .iter()
        .min_by(|a, b| {
            let da = (a.0 - hx).powi(2) + (a.1 - hy).powi(2);
            let db = (b.0 - hx).powi(2) + (b.1 - hy).powi(2);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })?;
    Some(*plane_origin + u_axis * best.0 + v_axis * best.1)
}

/// Build a blend face quad with a TorusSurface.
#[allow(clippy::too_many_arguments)]
fn build_blend_quad(
    edge_info: &EdgeInfo,
    trims: &HashMap<TrimKey, Point3>,
    faces: &[FaceInfo],
    torus: TorusSurface,
    vertex_cache: &mut HashMap<[i64; 3], VertexId>,
    new_topo: &mut Topology,
    new_geom: &mut GeometryStore,
    all_faces: &mut Vec<FaceId>,
) -> Option<()> {
    build_blend_quad_surface(
        edge_info,
        trims,
        faces,
        Box::new(torus),
        vertex_cache,
        new_topo,
        new_geom,
        all_faces,
    )
}

/// Build a blend face quad with any surface.
#[allow(clippy::too_many_arguments)]
fn build_blend_quad_surface(
    edge_info: &EdgeInfo,
    trims: &HashMap<TrimKey, Point3>,
    faces: &[FaceInfo],
    surface: Box<dyn Surface>,
    vertex_cache: &mut HashMap<[i64; 3], VertexId>,
    new_topo: &mut Topology,
    new_geom: &mut GeometryStore,
    all_faces: &mut Vec<FaceId>,
) -> Option<()> {
    let pa_s = trims.get(&(edge_info.v_start, edge_info.face_a))?;
    let pa_e = trims.get(&(edge_info.v_end, edge_info.face_a))?;
    let pb_s = trims.get(&(edge_info.v_start, edge_info.face_b))?;
    let pb_e = trims.get(&(edge_info.v_end, edge_info.face_b))?;

    let surf_idx = new_geom.add_surface(surface);
    let solid_center = compute_centroid(faces);
    let chamfer_center =
        Point3::from((pa_s.to_vec() + pa_e.to_vec() + pb_e.to_vec() + pb_s.to_vec()) * 0.25);
    let outward = chamfer_center - solid_center;
    let e1 = *pa_e - *pa_s;
    let e2 = *pb_s - *pa_s;
    let n = e1.cross(e2);

    let positions = if n.dot(outward) > 0.0 {
        vec![*pa_s, *pa_e, *pb_e, *pb_s]
    } else {
        vec![*pa_s, *pb_s, *pb_e, *pa_e]
    };

    let get_or_create =
        |cache: &mut HashMap<[i64; 3], VertexId>, topo: &mut Topology, pos: Point3| -> VertexId {
            let key = quantize(pos);
            *cache.entry(key).or_insert_with(|| topo.add_vertex(pos))
        };

    let verts: Vec<VertexId> = positions
        .iter()
        .map(|p| get_or_create(vertex_cache, new_topo, *p))
        .collect();

    let hes: Vec<HalfEdgeId> = verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
    let loop_id = new_topo.add_loop(&hes);
    let face_id = new_topo.add_face(loop_id, surf_idx, Orientation::Forward);
    all_faces.push(face_id);
    Some(())
}

/// Build a torus blend surface for a plane-cylinder edge.
fn build_plane_cylinder_torus(
    plane_surf: &dyn Surface,
    cyl_surf: &dyn Surface,
    plane_orientation: Orientation,
    radius: f64,
) -> Option<TorusSurface> {
    let plane = plane_surf.as_any().downcast_ref::<Plane>()?;
    let cyl = cyl_surf.as_any().downcast_ref::<CylinderSurface>()?;

    let axis = cyl.axis;
    let plane_normal = match plane_orientation {
        Orientation::Forward => *plane.normal_dir.as_ref(),
        Orientation::Reversed => -*plane.normal_dir.as_ref(),
    };

    // Project the plane onto the cylinder axis to get the intersection
    // point, then step `radius` along the axis *inward* (away from the
    // cap's outward normal). Previous code attempted this by moving
    // along `plane_normal` directly which is wrong when plane_normal and
    // axis point in opposite directions (e.g. the top cap of an
    // arc-extruded cylinder), and produced torus centers sitting OUTSIDE
    // the solid by 2*axial_offset.
    let plane_along_axis = plane_normal.dot(axis.as_ref());
    if plane_along_axis.abs() < 1e-12 {
        // Plane parallel to axis — no torus blend between them.
        return None;
    }

    let major_radius = cyl.radius + radius;
    if major_radius <= 0.0 {
        return None;
    }

    let to_plane = plane.signed_distance(&cyl.center);
    let cap_t = -to_plane / plane_along_axis;
    // Torus center sits on the cylinder axis, offset from the
    // plane-axis intersection by `radius` toward the solid interior
    // (opposite the plane's outward normal, projected onto the axis).
    let torus_t = cap_t - plane_along_axis.signum() * radius;
    let center_on_axis = cyl.center + torus_t * axis.as_ref();

    Some(TorusSurface {
        center: center_on_axis,
        axis,
        ref_dir: cyl.ref_dir,
        major_radius,
        minor_radius: radius,
    })
}

/// Build a torus blend surface for two coaxial cylinders (stepped shaft).
fn build_coaxial_cylinder_torus(
    surface_a: &dyn Surface,
    surface_b: &dyn Surface,
    radius: f64,
) -> Option<TorusSurface> {
    let cyl_a = surface_a.as_any().downcast_ref::<CylinderSurface>()?;
    let cyl_b = surface_b.as_any().downcast_ref::<CylinderSurface>()?;

    let (smaller, larger) = if cyl_a.radius < cyl_b.radius {
        (cyl_a, cyl_b)
    } else {
        (cyl_b, cyl_a)
    };

    let radius_step = larger.radius - smaller.radius;
    if radius < 1e-15 || radius > radius_step {
        return None;
    }

    let major_radius = smaller.radius + radius;
    let axis = smaller.axis;
    let d = larger.center - smaller.center;
    let t = d.dot(axis.as_ref());
    let center = smaller.center + t * axis.as_ref();

    Some(TorusSurface {
        center,
        axis,
        ref_dir: smaller.ref_dir,
        major_radius,
        minor_radius: radius,
    })
}
