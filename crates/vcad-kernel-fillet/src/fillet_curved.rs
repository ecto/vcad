//! Curved fillet support — torus blends for plane-cylinder and coaxial cylinder cases.

use std::collections::HashMap;
use vcad_kernel_geom::{CylinderSurface, GeometryStore, Plane, Surface, SurfaceKind, TorusSurface};
use vcad_kernel_math::Point3;
use vcad_kernel_primitives::BRepSolid;
use vcad_kernel_topo::{EdgeId, FaceId, HalfEdgeId, Orientation, ShellType, Topology, VertexId};

use crate::rolling_ball::rolling_ball_blend;
use crate::topology::{compute_centroid, compute_face_normal, extract_edges, extract_faces, pair_twin_half_edges, quantize, CurvedFaceInfo, EdgeInfo, FaceInfo};
use crate::trim::{build_vertex_faces, compute_trim_vertices, TrimKey};
use crate::fillet_planar::build_plane_plane_blend;
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
                        edge_info, fa, fb, &trims, &faces, radius, brep,
                        &mut vertex_cache, &mut new_topo, &mut new_geom, &mut all_faces,
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
                        edge_info, &trims, &faces, torus,
                        &mut vertex_cache, &mut new_topo, &mut new_geom, &mut all_faces,
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
                if let Some(torus) = build_coaxial_cylinder_torus(
                    surface_a.as_ref(),
                    surface_b.as_ref(),
                    radius,
                ) {
                    if let Some(()) = build_blend_quad(
                        edge_info, &trims, &faces, torus,
                        &mut vertex_cache, &mut new_topo, &mut new_geom, &mut all_faces,
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
                            edge_info, &trims, &faces, Box::new(bspline),
                            &mut vertex_cache, &mut new_topo, &mut new_geom, &mut all_faces,
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
                            reason: format!(
                                "rolling ball blend failed for {:?} edge",
                                case
                            ),
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
    build_blend_quad_surface(edge_info, trims, faces, Box::new(torus), vertex_cache, new_topo, new_geom, all_faces)
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
    let chamfer_center = Point3::from(
        (pa_s.coords + pa_e.coords + pb_e.coords + pb_s.coords) * 0.25,
    );
    let outward = chamfer_center - solid_center;
    let e1 = *pa_e - *pa_s;
    let e2 = *pb_s - *pa_s;
    let n = e1.cross(&e2);

    let positions = if n.dot(&outward) > 0.0 {
        vec![*pa_s, *pa_e, *pb_e, *pb_s]
    } else {
        vec![*pa_s, *pb_s, *pb_e, *pa_e]
    };

    let get_or_create = |cache: &mut HashMap<[i64; 3], VertexId>,
                         topo: &mut Topology,
                         pos: Point3|
     -> VertexId {
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

    let to_plane = plane.signed_distance(&cyl.center);
    let plane_along_axis = plane_normal.dot(axis.as_ref());

    let major_radius = cyl.radius + radius;
    if major_radius <= 0.0 {
        return None;
    }

    let torus_center = cyl.center - (to_plane - radius * plane_along_axis.signum()) * plane_normal;
    let axis_param = (torus_center - cyl.center).dot(axis.as_ref());
    let center_on_axis = cyl.center + axis_param * axis.as_ref();

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
