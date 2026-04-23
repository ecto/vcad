//! Curved fillet support — torus blends for plane-cylinder and coaxial cylinder cases.

use std::cell::RefCell;
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

/// What happened at a single candidate junction during vertex-blend
/// construction. Emitted one-per-vertex that `build_spherical_vertex_blends`
/// considered.
#[derive(Debug, Clone)]
pub struct JunctionTrace {
    /// The BRep vertex at the junction.
    pub vertex: VertexId,
    /// World-space position of the vertex.
    pub vertex_pos: Point3,
    /// Number of target (to-be-filleted) edges incident to this vertex.
    pub n_target_edges: usize,
    /// Number of non-target edges incident (typically seams).
    pub n_seam_edges: usize,
    /// Outcome: why the patch was built, or why it was rejected.
    pub outcome: JunctionOutcome,
}

/// Outcome of attempting to build a spherical vertex blend at one junction.
#[derive(Debug, Clone)]
pub enum JunctionOutcome {
    /// Skipped — not exactly 2 filleted edges at this vertex.
    SkippedWrongEdgeCount,
    /// Skipped — one or both filleted edges wasn't plane-cylinder.
    SkippedNotPlaneCylinder,
    /// Skipped — the two cap faces disagreed (mixed cap normal).
    SkippedMixedCapNormal,
    /// Skipped — third edge wasn't a cylinder-cylinder seam.
    SkippedNoCylCylSeam,
    /// Skipped — one of the cylinder radii was ≤ fillet radius.
    SkippedCylRadiusTooSmall {
        /// The (cyl.radius - fillet_radius) values for each cylinder;
        /// any ≤ 0 entry triggers the skip.
        radii: Vec<f64>,
    },
    /// Skipped — the two offset circles don't intersect.
    SkippedCirclesDisjoint,
    /// Skipped — ball is coincident with a cylinder axis (radial undefined).
    SkippedRadialDegenerate,
    /// Skipped — two of the three tangent points collapsed to the same vertex.
    SkippedDegenerateTriangle,
    /// Successfully built a spherical patch.
    BuiltPatch {
        /// Rolling ball center.
        ball_center: Point3,
        /// Tangent point on the cap plane.
        tan_cap: Point3,
        /// Tangent points on the two cylinder surfaces.
        tan_cyls: [Point3; 2],
    },
}

/// A collected trace of every junction considered during one fillet call.
#[derive(Debug, Default, Clone)]
pub struct FilletTrace {
    /// Per-junction entries, in no particular order.
    pub junctions: Vec<JunctionTrace>,
}

thread_local! {
    static TRACE: RefCell<Option<FilletTrace>> = const { RefCell::new(None) };
}

fn trace_enabled() -> bool {
    TRACE.with(|t| t.borrow().is_some())
}

fn trace_push(entry: JunctionTrace) {
    TRACE.with(|t| {
        if let Some(trace) = t.borrow_mut().as_mut() {
            trace.junctions.push(entry);
        }
    })
}

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
    let (brep, results, _) = fillet_edges_detailed_with_trace(brep, edge_ids, radius, false);
    (brep, results)
}

/// Like `fillet_edges_detailed`, but also returns a per-junction trace
/// when `capture_trace` is true. Used by diagnostics and tests to inspect
/// what happened at each convex arc-to-arc junction. Pass `false` for the
/// normal path — the trace buffer isn't allocated.
pub fn fillet_edges_detailed_with_trace(
    brep: &BRepSolid,
    edge_ids: &[EdgeId],
    radius: f64,
    capture_trace: bool,
) -> (BRepSolid, Vec<FilletResult>, FilletTrace) {
    let trace_slot = if capture_trace {
        Some(FilletTrace::default())
    } else {
        None
    };
    TRACE.with(|t| *t.borrow_mut() = trace_slot);
    let (brep, results) = fillet_edges_detailed_inner(brep, edge_ids, radius);
    let trace = TRACE.with(|t| t.borrow_mut().take()).unwrap_or_default();
    (brep, results, trace)
}

fn fillet_edges_detailed_inner(
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

    // Classify convex plane-cyl-cyl junctions up-front so downstream
    // stages share a single source of truth for patch tangent points.
    let junction_plans = plan_convex_junction_blends(brep, &edges, &target_edges, radius);

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
    //
    // Patches are currently topology-isolated (3 floating edges per
    // patch + 6-vert hex loop at each junction that the harness's
    // loop-to-face reconciliation traces to the tan_cyl / V_axial
    // mismatch on each cylinder). The fix has to shift the cylinder
    // face outer loop to END at tan_cyl instead of V_axial — a naïve
    // "add a bridge quad on the cylinder surface" approach does NOT
    // work because the existing cylinder face already claims the
    // V_axial seam column, so the bridge overlaps and the tessellator
    // produces non-manifold edges / bigger holes. The correct fix is
    // topology surgery on the cylinder face's outer loop; deferred.
    build_spherical_vertex_blends_from_plans(
        &junction_plans,
        &faces,
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

/// Result of classifying one candidate convex junction: everything
/// downstream stages (trim override + sphere patch construction) need
/// to know about the vertex.
struct JunctionPlan {
    vertex: VertexId,
    cap_face: FaceId,
    cyl_faces: [FaceId; 2],
    ball_center: Point3,
    tan_cap: Point3,
    tan_cyls: [Point3; 2],
    cap_outward_normal: Vec3,
}

/// Scan every candidate vertex and return the plans for those that
/// qualify as convex plane-cyl-cyl junctions. Emits trace entries
/// along the way if trace capture is active.
fn plan_convex_junction_blends(
    brep: &BRepSolid,
    edges: &[EdgeInfo],
    target_edges: &[&EdgeInfo],
    radius: f64,
) -> Vec<JunctionPlan> {
    let topo = &brep.topology;
    let geom = &brep.geometry;

    let mut target_by_vertex: HashMap<VertexId, Vec<&EdgeInfo>> = HashMap::new();
    for edge in target_edges {
        target_by_vertex.entry(edge.v_start).or_default().push(edge);
        target_by_vertex.entry(edge.v_end).or_default().push(edge);
    }

    let mut all_by_vertex: HashMap<VertexId, Vec<&EdgeInfo>> = HashMap::new();
    for edge in edges {
        all_by_vertex.entry(edge.v_start).or_default().push(edge);
        all_by_vertex.entry(edge.v_end).or_default().push(edge);
    }

    let mut plans = Vec::new();

    for (v_id, tgt_edges) in &target_by_vertex {
        let v_pos_for_trace = topo.vertices[*v_id].point;
        let incident_count = all_by_vertex.get(v_id).map(|v| v.len()).unwrap_or(0);
        let seam_count = incident_count.saturating_sub(tgt_edges.len());
        let emit = |outcome: JunctionOutcome| {
            if trace_enabled() {
                trace_push(JunctionTrace {
                    vertex: *v_id,
                    vertex_pos: v_pos_for_trace,
                    n_target_edges: tgt_edges.len(),
                    n_seam_edges: seam_count,
                    outcome,
                });
            }
        };

        if tgt_edges.len() != 2 {
            emit(JunctionOutcome::SkippedWrongEdgeCount);
            continue;
        }

        let mut plane_normal: Option<Vec3> = None;
        let mut cyls: Vec<CylinderSurface> = Vec::new();
        let mut cyl_faces: Vec<FaceId> = Vec::new();
        let mut cap_face_id: Option<FaceId> = None;
        let mut skip_reason: Option<JunctionOutcome> = None;
        for e in tgt_edges {
            let sa = &geom.surfaces[topo.faces[e.face_a].surface_index];
            let sb = &geom.surfaces[topo.faces[e.face_b].surface_index];
            let (plane_face, plane_surf, cyl_face, cyl_surf) =
                match (sa.surface_type(), sb.surface_type()) {
                    (SurfaceKind::Plane, SurfaceKind::Cylinder) => (e.face_a, sa, e.face_b, sb),
                    (SurfaceKind::Cylinder, SurfaceKind::Plane) => (e.face_b, sb, e.face_a, sa),
                    _ => {
                        skip_reason = Some(JunctionOutcome::SkippedNotPlaneCylinder);
                        break;
                    }
                };
            let plane = match plane_surf.as_any().downcast_ref::<Plane>() {
                Some(p) => p,
                None => {
                    skip_reason = Some(JunctionOutcome::SkippedNotPlaneCylinder);
                    break;
                }
            };
            let cyl = match cyl_surf.as_any().downcast_ref::<CylinderSurface>() {
                Some(c) => c.clone(),
                None => {
                    skip_reason = Some(JunctionOutcome::SkippedNotPlaneCylinder);
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
                    if n.dot(outward) < 1.0 - 1e-6 {
                        skip_reason = Some(JunctionOutcome::SkippedMixedCapNormal);
                        break;
                    }
                }
            }
            cyls.push(cyl);
            cyl_faces.push(cyl_face);
        }
        if let Some(r) = skip_reason {
            emit(r);
            continue;
        }
        let plane_normal = match plane_normal {
            Some(n) => n,
            None => {
                emit(JunctionOutcome::SkippedNotPlaneCylinder);
                continue;
            }
        };
        let cap_face = match cap_face_id {
            Some(f) => f,
            None => {
                emit(JunctionOutcome::SkippedNotPlaneCylinder);
                continue;
            }
        };
        if cyls.len() != 2 || cyl_faces.len() != 2 {
            emit(JunctionOutcome::SkippedNotPlaneCylinder);
            continue;
        }

        let incident = match all_by_vertex.get(v_id) {
            Some(v) => v,
            None => {
                emit(JunctionOutcome::SkippedNoCylCylSeam);
                continue;
            }
        };
        if incident.len() < 3 {
            emit(JunctionOutcome::SkippedNoCylCylSeam);
            continue;
        }
        let is_target = |e: &&EdgeInfo| tgt_edges.iter().any(|t| t.edge_id == e.edge_id);
        let seam_edges: Vec<&&EdgeInfo> = incident.iter().filter(|e| !is_target(e)).collect();
        let seam_connects_cyls = seam_edges.iter().any(|e| {
            let sa = &geom.surfaces[topo.faces[e.face_a].surface_index];
            let sb = &geom.surfaces[topo.faces[e.face_b].surface_index];
            sa.surface_type() == SurfaceKind::Cylinder && sb.surface_type() == SurfaceKind::Cylinder
        });
        if !seam_connects_cyls {
            emit(JunctionOutcome::SkippedNoCylCylSeam);
            continue;
        }

        let v_pos = topo.vertices[*v_id].point;
        let cap_center_t = v_pos + (-plane_normal) * radius;

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
            emit(JunctionOutcome::SkippedCylRadiusTooSmall {
                radii: offset_radii.clone(),
            });
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
            None => {
                emit(JunctionOutcome::SkippedCirclesDisjoint);
                continue;
            }
        };

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
            tan_cyls.push(c.center + *c.axis.as_ref() * along + radial_unit * c.radius);
            let _ = r;
        }
        if tan_cyls.len() != 2 {
            emit(JunctionOutcome::SkippedRadialDegenerate);
            continue;
        }

        emit(JunctionOutcome::BuiltPatch {
            ball_center,
            tan_cap,
            tan_cyls: [tan_cyls[0], tan_cyls[1]],
        });
        plans.push(JunctionPlan {
            vertex: *v_id,
            cap_face,
            cyl_faces: [cyl_faces[0], cyl_faces[1]],
            ball_center,
            tan_cap,
            tan_cyls: [tan_cyls[0], tan_cyls[1]],
            cap_outward_normal: plane_normal,
        });
    }

    plans
}

/// Build spherical vertex-blend patches from the pre-classified plans.
/// Uses the shared `vertex_cache` so each patch corner collapses to the
/// same `VertexId` as the adjacent cylinder-face trim and torus-blend
/// quad corner.
fn build_spherical_vertex_blends_from_plans(
    plans: &[JunctionPlan],
    faces: &[FaceInfo],
    vertex_cache: &mut HashMap<[i64; 3], VertexId>,
    new_topo: &mut Topology,
    new_geom: &mut GeometryStore,
    all_faces: &mut Vec<FaceId>,
) {
    let solid_centroid = compute_centroid(faces);
    for plan in plans {
        let get_or_create =
            |cache: &mut HashMap<[i64; 3], VertexId>, topo: &mut Topology, p: Point3| -> VertexId {
                let key = quantize(p);
                *cache.entry(key).or_insert_with(|| topo.add_vertex(p))
            };
        let v_cap = get_or_create(vertex_cache, new_topo, plan.tan_cap);
        let v_c1 = get_or_create(vertex_cache, new_topo, plan.tan_cyls[0]);
        let v_c2 = get_or_create(vertex_cache, new_topo, plan.tan_cyls[1]);
        if v_cap == v_c1 || v_c1 == v_c2 || v_cap == v_c2 {
            continue;
        }

        let sphere = SphereSurface {
            center: plan.ball_center,
            radius: (plan.tan_cap - plan.ball_center).norm(),
            ref_dir: Dir3::new_normalize(plan.tan_cap - plan.ball_center),
            axis: Dir3::new_normalize(-plan.cap_outward_normal),
        };

        // Winding: the sphere patch sits on the cap's outward side,
        // so its CCW normal must point in the same direction as the
        // cap's outward normal. Compare the candidate CCW against
        // `plan.cap_outward_normal` directly (the cap face's true
        // outward) rather than `v_pos - solid_centroid`, which mixes
        // in the radial direction and flips the decision for patches
        // near the solid's center of mass.
        let _ = solid_centroid;
        let e1 = plan.tan_cyls[0] - plan.tan_cap;
        let e2 = plan.tan_cyls[1] - plan.tan_cap;
        let n = e1.cross(e2);
        let verts: [VertexId; 3] = if n.dot(plan.cap_outward_normal) > 0.0 {
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

/// For every junction plan, rewrite the trim table so all participating
/// faces resolve V — and any intermediate arc-sample vertex in the
/// "clip wedge" near V — to the rolling-ball tangent points.
///
/// An intermediate is in the clip wedge when its angle on the cylinder
/// lies strictly between `tan_cyl_angle` and `V_angle` (short arc).
/// Those intermediates' trims for THIS cylinder face and the SHARED
/// cap face both get snapped to the tangent endpoints — collapsing
/// the sliver of arc beyond tan_cyl into a zero-width region. The
/// two-three micro torus-blend segments that live there produce
/// degenerate quads that `build_blend_quad` rejects (via the
/// vertex-equality early-out), so the net effect is that the cap-cyl
/// arc is visually resampled to end at tan_cyl, without the pipeline
/// needing any new "skip this segment" logic.
fn snap_trims_at_junctions(
    brep: &BRepSolid,
    plans: &[JunctionPlan],
    trims: &mut HashMap<TrimKey, Point3>,
) {
    let topo = &brep.topology;
    let geom = &brep.geometry;

    // `(u, v)` of a 3D point on a cylinder's surface.
    let uv_on = |pt: Point3, cyl: &CylinderSurface| -> (f64, f64) {
        let axis = *cyl.axis.as_ref();
        let ref_dir = *cyl.ref_dir.as_ref();
        let y_dir = axis.cross(ref_dir);
        let d = pt - cyl.center;
        let u = d.dot(y_dir).atan2(d.dot(ref_dir));
        let u = if u < 0.0 {
            u + 2.0 * std::f64::consts::PI
        } else {
            u
        };
        (u, d.dot(axis))
    };

    // Shortest signed arc from `a` to `b` in `(-π, π]`.
    let signed_arc = |a: f64, b: f64| -> f64 {
        let mut d = b - a;
        while d > std::f64::consts::PI {
            d -= 2.0 * std::f64::consts::PI;
        }
        while d <= -std::f64::consts::PI {
            d += 2.0 * std::f64::consts::PI;
        }
        d
    };

    for plan in plans {
        // Override V's own trim for all three participating faces.
        trims.insert((plan.vertex, plan.cap_face), plan.tan_cap);
        trims.insert((plan.vertex, plan.cyl_faces[0]), plan.tan_cyls[0]);
        trims.insert((plan.vertex, plan.cyl_faces[1]), plan.tan_cyls[1]);

        let v_pos = topo.vertices[plan.vertex].point;

        for (idx, &cyl_face) in plan.cyl_faces.iter().enumerate() {
            let surf = &geom.surfaces[topo.faces[cyl_face].surface_index];
            let cyl = match surf.as_any().downcast_ref::<CylinderSurface>() {
                Some(c) => c,
                None => continue,
            };

            let (tan_u, _) = uv_on(plan.tan_cyls[idx], cyl);
            let (v_u, _) = uv_on(v_pos, cyl);
            let clip_span = signed_arc(tan_u, v_u);
            if clip_span.abs() < 1e-6 {
                continue;
            }

            let outer_loop = topo.faces[cyl_face].outer_loop;
            for he in topo.loop_half_edges(outer_loop) {
                let vid = topo.half_edges[he].origin;
                if vid == plan.vertex {
                    continue;
                }
                let vpos = topo.vertices[vid].point;
                let (vu, _) = uv_on(vpos, cyl);
                let offset = signed_arc(tan_u, vu);
                if offset.signum() == clip_span.signum()
                    && offset.abs() > 1e-6
                    && offset.abs() < clip_span.abs() - 1e-6
                {
                    // Snap BOTH the cyl-face trim (to tan_cyl) and the
                    // shared cap-face trim (to tan_cap). Both tan points
                    // share the cap's z; consecutive clipped ints all
                    // collapse to the same VertexId via the cache.
                    trims.insert((vid, cyl_face), plan.tan_cyls[idx]);
                    trims.insert((vid, plan.cap_face), plan.tan_cap);
                }
            }
        }
    }
}

/// Close the hexagonal armpit gap at every convex junction by adding
/// a quad face on each cylinder's surface between the sphere-patch
/// tan_cyl column and the original V_axial seam column.
///
/// The junction has a "bottom" plan (V on the z=0 cap) and a "top"
/// plan (V on the z=h cap) that share the same (cyl_face, V.xy). We
/// pair them and emit one quad per cylinder with corners:
///     bot_tan_cyl  (at cyl v_min)
///     bot_V_axial  (at cyl v_min, axial-shifted from V)
///     top_V_axial  (at cyl v_max)
///     top_tan_cyl  (at cyl v_max)
/// All four points lie on the cylinder's surface, so the quad re-uses
/// the cylinder's CylinderSurface definition.
fn build_cylinder_bridge_quads(
    brep: &BRepSolid,
    plans: &[JunctionPlan],
    radius: f64,
    vertex_cache: &mut HashMap<[i64; 3], VertexId>,
    new_topo: &mut Topology,
    new_geom: &mut GeometryStore,
    all_faces: &mut Vec<FaceId>,
) {
    // Group plans by (cyl_face, quantized V.xy). Each group should end
    // up with exactly two entries — bottom junction + top junction.
    type Key = (FaceId, [i64; 2]);
    let mut groups: HashMap<Key, Vec<&JunctionPlan>> = HashMap::new();
    for plan in plans {
        let v_pos = brep.topology.vertices[plan.vertex].point;
        let xy_key = [
            (v_pos.x * 1000.0).round() as i64,
            (v_pos.y * 1000.0).round() as i64,
        ];
        for (cyl_idx, &cyl_face) in plan.cyl_faces.iter().enumerate() {
            let _ = cyl_idx;
            groups.entry((cyl_face, xy_key)).or_default().push(plan);
        }
    }

    for ((cyl_face, _xy_key), group) in &groups {
        if group.len() != 2 {
            continue;
        }
        // Pull the cylinder surface + its axis so we know which way
        // is "up" along the axis and can project z-levels correctly.
        let surf = &brep.geometry.surfaces[brep.topology.faces[*cyl_face].surface_index];
        let cyl = match surf.as_any().downcast_ref::<CylinderSurface>() {
            Some(c) => c.clone(),
            None => continue,
        };
        let axis = *cyl.axis.as_ref();

        // Order the two plans by axial position of their vertex —
        // smaller axial = v_min side, larger = v_max side.
        let mut sorted: Vec<&&JunctionPlan> = group.iter().collect();
        sorted.sort_by(|a, b| {
            let va = (brep.topology.vertices[a.vertex].point - cyl.center).dot(axis);
            let vb = (brep.topology.vertices[b.vertex].point - cyl.center).dot(axis);
            va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal)
        });
        let bot_plan = sorted[0];
        let top_plan = sorted[1];

        // Find which cyl index each plan is on for this cyl_face.
        let bot_tan_idx = if bot_plan.cyl_faces[0] == *cyl_face { 0 } else { 1 };
        let top_tan_idx = if top_plan.cyl_faces[0] == *cyl_face { 0 } else { 1 };
        let bot_tan = bot_plan.tan_cyls[bot_tan_idx];
        let top_tan = top_plan.tan_cyls[top_tan_idx];

        // V axial positions: the cylinder face's trim shifts V by
        // ±radius along the axis. bot's V goes +axis·r (toward
        // interior), top's goes -axis·r.
        let bot_v_pos = brep.topology.vertices[bot_plan.vertex].point;
        let top_v_pos = brep.topology.vertices[top_plan.vertex].point;
        let bot_v_axial = bot_v_pos + axis * radius;
        let top_v_axial = top_v_pos - axis * radius;

        let get_or_create =
            |cache: &mut HashMap<[i64; 3], VertexId>, topo: &mut Topology, p: Point3| -> VertexId {
                let key = quantize(p);
                *cache.entry(key).or_insert_with(|| topo.add_vertex(p))
            };
        let v_bt = get_or_create(vertex_cache, new_topo, bot_tan);
        let v_bv = get_or_create(vertex_cache, new_topo, bot_v_axial);
        let v_tv = get_or_create(vertex_cache, new_topo, top_v_axial);
        let v_tt = get_or_create(vertex_cache, new_topo, top_tan);

        if v_bt == v_bv || v_bv == v_tv || v_tv == v_tt || v_tt == v_bt {
            continue;
        }

        // Winding: the solid is inside the cylinder (convex arc, so
        // solid center is on the axis side). Outward normal of this
        // quad should point AWAY from the axis — i.e. radially
        // outward at the quad's centroid.
        let centroid = Point3::from(
            (bot_tan.to_vec() + bot_v_axial.to_vec() + top_v_axial.to_vec() + top_tan.to_vec())
                * 0.25,
        );
        let radial = {
            let d = centroid - cyl.center;
            let along = d.dot(axis);
            d - axis * along
        };
        let e1 = bot_v_axial - bot_tan;
        let e2 = top_tan - bot_tan;
        let n = e1.cross(e2);
        let verts: [VertexId; 4] = if n.dot(radial) > 0.0 {
            [v_bt, v_bv, v_tv, v_tt]
        } else {
            [v_bt, v_tt, v_tv, v_bv]
        };

        let surf_idx = new_geom.add_surface(Box::new(cyl));
        let hes: Vec<HalfEdgeId> = verts.iter().map(|&v| new_topo.add_half_edge(v)).collect();
        let loop_id = new_topo.add_loop(&hes);
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
    let best = candidates.iter().min_by(|a, b| {
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

    // Reject degenerate quads where both start- or end-side corners
    // have collapsed to the same position (happens by design at
    // clipped junction sliver segments — see `snap_trims_at_junctions`).
    if (*pa_s - *pa_e).norm() < 1e-6 && (*pb_s - *pb_e).norm() < 1e-6 {
        return Some(());
    }

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
